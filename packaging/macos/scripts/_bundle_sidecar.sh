#!/usr/bin/env bash
# Shared helper: bundles pi-sidecar resources (JS bundle, manifest.json,
# dual-arch Node binaries) into Busytok.app/Contents/Resources/pi-sidecar/.
#
# Source this from package_dmg.sh. Expects PROJECT_ROOT + SIDECAR_NODE_VERSION
# to be set (from _release_vars.sh).
#
# Usage:
#   bundle_sidecar_resources_into_app APP_BUNDLE_PATH
#   sign_sidecar_node_binaries APP_BUNDLE_PATH DEVELOPER_ID [TIMESTAMP_FLAG]
#
# Spec §345-390: .app structure, manifest schema, dual-arch, signing order.

# Architecture mapping: <upstream_archive_token> <app_internal_dir_name>
# Node.js release archives use darwin-arm64 / darwin-x64 (NOT
# aarch64-apple-darwin / x86_64-apple-darwin). The app's internal layout
# uses aarch64 / x86_64 (matching std::env::consts::ARCH on macOS).
# These two layers MUST NOT be conflated — see P0 review finding.
NODE_ARCHES=(
    "darwin-arm64 aarch64"
    "darwin-x64 x86_64"
)

# Generate manifest.json with the current protocol_version from the Rust
# source. Reads PROTOCOL_VERSION from
# crates/busytok-subagent/src/sidecar/protocol.rs and emits a JSON manifest
# conforming to busytok_config::SidecarManifest (Task 1).
generate_sidecar_manifest() {
    local out_dir="$1"
    local protocol_version
    # Extract the integer literal from: pub const PROTOCOL_VERSION: u32 = 1;
    protocol_version=$(grep -E '^\s*pub const PROTOCOL_VERSION' \
        "$PROJECT_ROOT/crates/busytok-subagent/src/sidecar/protocol.rs" \
        | grep -oE '= [0-9]+' | tr -d '= ')
    if [ -z "$protocol_version" ]; then
        echo "  ERROR: could not extract PROTOCOL_VERSION from protocol.rs"
        return 1
    fi
    cat > "$out_dir/manifest.json" <<EOF
{
  "version": "1",
  "protocol_version": ${protocol_version},
  "bundle": "pi-sidecar.bundle.js",
  "node_runtime_version": "${SIDECAR_NODE_VERSION}"
}
EOF
    if [ ! -f "$out_dir/manifest.json" ]; then
        echo "  ERROR: manifest write failed"
        return 1
    fi
    echo "  generated manifest.json (protocol_version=${protocol_version}, node=${SIDECAR_NODE_VERSION})"
}

# Download a single Node binary tarball for the given arch + extract the
# `bin/node` file to <staging>/node/<app_dir>/node. Caches by version+arch.
# Args: <upstream_archive_token> <app_internal_dir_name> <staging>
download_node_binary() {
    local upstream_token="$1"
    local app_dir="$2"
    local staging="$3"
    local version="${SIDECAR_NODE_VERSION}"
    local dest_dir="$staging/node/$app_dir"
    local cached="$staging/cache/node-v${version}-${upstream_token}.tar.gz"

    mkdir -p "$dest_dir" "$staging/cache" \
        || { echo "  ERROR: mkdir failed"; return 1; }

    if [ -f "$cached" ]; then
        echo "  node v${version} ${upstream_token}: cached"
    else
        local url="https://nodejs.org/dist/v${version}/node-v${version}-${upstream_token}.tar.gz"
        echo "  downloading $url"
        if ! curl -fsSL "$url" -o "$cached"; then
            echo "  ERROR: download failed for $url"
            return 1
        fi
    fi

    # Extract only bin/node from the tarball (the full archive is ~90MB).
    # Note: this function is invoked via `download_node_binary ... || return 1`,
    # which disables `set -e` inside the body — so each command must check
    # its own exit status explicitly.
    if ! tar -xzf "$cached" -C "$dest_dir" \
        --strip-components=1 \
        "node-v${version}-${upstream_token}/bin/node"; then
        echo "  ERROR: tar extraction failed for $cached"
        return 1
    fi
    if [ ! -f "$dest_dir/bin/node" ]; then
        echo "  ERROR: bin/node not found after extraction"
        return 1
    fi
    mv "$dest_dir/bin/node" "$dest_dir/node" || { echo "  ERROR: mv bin/node failed"; return 1; }
    rmdir "$dest_dir/bin" 2>/dev/null || true
    chmod +x "$dest_dir/node" || { echo "  ERROR: chmod node failed"; return 1; }
    echo "  extracted node v${version} ${upstream_token} -> $dest_dir/node"
}

# Copy pi-sidecar.bundle.js + manifest.json + both node arches into the
# .app's Resources/pi-sidecar/ directory.
bundle_sidecar_resources_into_app() {
    local app_bundle
    app_bundle="$(_absolute_app_bundle_path "$1")"
    local sidecar_dir="$app_bundle/Contents/Resources/pi-sidecar"
    local bundle_src="$PROJECT_ROOT/apps/pi-sidecar/dist/pi-sidecar.bundle.js"

    echo "Bundling pi-sidecar resources into $app_bundle..."

    # 1. ALWAYS rebuild the JS bundle from current source. Conditional
    # build (only if missing) would silently package stale dist/ output,
    # breaking release determinism (P1 review finding). The pi-sidecar
    # package is small (~100ms build); unconditional rebuild is cheap.
    echo "  building pi-sidecar bundle (unconditional, fresh from source)..."
    ( cd "$PROJECT_ROOT/apps/pi-sidecar" && pnpm run build ) \
        || { echo "  ERROR: pi-sidecar build failed"; return 1; }
    if [ ! -f "$bundle_src" ]; then
        echo "  ERROR: pi-sidecar.bundle.js not produced by build"
        return 1
    fi

    mkdir -p "$sidecar_dir/node/aarch64" "$sidecar_dir/node/x86_64" \
        || { echo "  ERROR: mkdir failed"; return 1; }

    # 2. Copy the JS bundle.
    cp "$bundle_src" "$sidecar_dir/pi-sidecar.bundle.js" || { echo "  ERROR: cp bundle failed"; return 1; }
    echo "  bundled pi-sidecar.bundle.js"

    # 2b. Install Pi SDK runtime dependency via npm into a temp directory,
    # then copy into the bundle.  This is the ONLY reliable way to get all
    # transitive dependencies — pnpm's virtual store makes manual copying
    # of transitive deps fragile (each dep's own deps live in a separate
    # .pnpm/<dep>@<hash>/node_modules/ directory).  npm's install nests
    # all transitive deps inside @earendil-works/pi-coding-agent/node_modules/,
    # so copying the package dir brings everything in one pass.
    local pi_sdk_pkg="@earendil-works/pi-coding-agent"
    # Single source of truth: read the SDK version from package.json instead
    # of hardcoding it here (avoids drift between bundle script and declared
    # dep when the SDK is upgraded).
    local pi_sdk_version
    pi_sdk_version=$(sed -n 's|.*"@earendil-works/pi-coding-agent"[[:space:]]*:[[:space:]]*"\([^"]*\)".*|\1|p' \
        "$PROJECT_ROOT/apps/pi-sidecar/package.json" | head -1)
    if [ -z "$pi_sdk_version" ]; then
        echo "  ERROR: could not extract $pi_sdk_pkg version from package.json"
        return 1
    fi
    local pi_sdk_dst="$sidecar_dir/node_modules/@earendil-works/pi-coding-agent"
    local npm_staging
    npm_staging="$(mktemp -d "${TMPDIR:-/tmp}/busytok-pi-sdk-npm.XXXXXX")" \
        || { echo "  ERROR: mktemp npm staging failed"; return 1; }

    echo "  installing $pi_sdk_pkg@$pi_sdk_version via npm..."
    # --no-package-lock: the staging dir is ephemeral. A repo-owned
    # lockfile would add maintenance burden for marginal determinism
    # gain in 0.x. Revisit with a committed lockfile at 1.0.
    ( cd "$npm_staging" \
        && npm init -y --silent 2>/dev/null \
        && npm install "${pi_sdk_pkg}@${pi_sdk_version}" --omit=dev --no-package-lock --silent 2>&1
    ) || { rm -rf "$npm_staging"; echo "  ERROR: npm install ${pi_sdk_pkg} failed"; return 1; }

    local npm_mod="$npm_staging/node_modules/@earendil-works/pi-coding-agent"
    if [ ! -d "$npm_mod" ]; then
        rm -rf "$npm_staging"
        echo "  ERROR: $pi_sdk_pkg not found after npm install"
        return 1
    fi

    mkdir -p "$(dirname "$pi_sdk_dst")"
    # rm -rf first: cp -R src dst/ would nest src inside dst if dst already
    # exists (e.g. when re-running the bundler on an existing .app during
    # local debugging). Release builds always start fresh, but this makes
    # re-runs safe.
    rm -rf "$pi_sdk_dst"
    cp -R "$npm_mod" "$pi_sdk_dst" \
        || { rm -rf "$npm_staging"; echo "  ERROR: cp pi-sdk from npm staging failed"; return 1; }
    rm -rf "$npm_staging"
    echo "  bundled pi-coding-agent SDK + all transitive deps ($(du -sh "$pi_sdk_dst" 2>/dev/null | cut -f1))"

    # 3. Generate manifest.json (Task 1 schema).
    generate_sidecar_manifest "$sidecar_dir" || return 1

    # 4. Download + extract both Node arches into staging, then copy.
    # Iterate NODE_ARCHES: each entry is "<upstream_token> <app_dir>".
    local staging
    staging="$(mktemp -d "${TMPDIR:-/tmp}/busytok-node-staging.XXXXXX")" \
        || { echo "  ERROR: mktemp staging failed"; return 1; }
    # shellcheck disable=SC2064
    trap "rm -rf '$staging'" RETURN

    local entry upstream_token app_dir
    for entry in "${NODE_ARCHES[@]}"; do
        # shellcheck disable=SC2086
        set -- $entry
        upstream_token="$1"
        app_dir="$2"
        download_node_binary "$upstream_token" "$app_dir" "$staging" || return 1
        cp "$staging/node/$app_dir/node" "$sidecar_dir/node/$app_dir/node" \
            || { echo "  ERROR: cp $app_dir node failed"; return 1; }
        chmod +x "$sidecar_dir/node/$app_dir/node" \
            || { echo "  ERROR: chmod $app_dir node failed"; return 1; }
    done
    echo "  bundled node ${SIDECAR_NODE_VERSION} (aarch64 + x86_64)"

    echo "Sidecar bundling complete."
}

# Sign the nested node binaries with the sidecar-node.plist entitlements.
# MUST run BEFORE the outer .app re-seal (spec §375-381 signing order).
sign_sidecar_node_binaries() {
    local app_bundle
    app_bundle="$(_absolute_app_bundle_path "$1")"
    local developer_id="$2"
    local timestamp_flag="${3:---timestamp}"
    local entitlements="$PROJECT_ROOT/packaging/macos/entitlements/sidecar-node.plist"
    local sidecar_dir="$app_bundle/Contents/Resources/pi-sidecar"

    echo "Signing sidecar node binaries with $developer_id..."
    for arch in aarch64 x86_64; do
        local node_path="$sidecar_dir/node/$arch/node"
        if [ -f "$node_path" ]; then
            codesign --sign "$developer_id" \
                --entitlements "$entitlements" \
                --options runtime \
                $timestamp_flag \
                --force \
                "$node_path"
            echo "  signed node ($arch)"
        else
            echo "  WARNING: node binary not found at $node_path — skipping sign"
        fi
    done
}
