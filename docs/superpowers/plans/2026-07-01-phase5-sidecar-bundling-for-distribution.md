# Phase 5: Sidecar Bundling for Distribution — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Bundle the pi-sidecar JS bundle, a generated `manifest.json`, and dual-arch Node binaries into the macOS `.app`'s `Contents/Resources/pi-sidecar/` directory, persist the resolved resource path as `runtime_dir` in `settings.toml` on GUI startup so the daemon can locate the sidecar without `current_exe()` guessing, sign the nested Node binary with `allow-jit` entitlements, and extend the release smoke test to verify the bundle contract — closing Gap 4 (Sidecar Bundling) so that a fresh install passes all 10 doctor checks and `busytok delegate` spawns the sidecar end-to-end.

**Architecture:** The existing `_bundle_helpers.sh` shell-helper pattern is extended with a new `bundle_sidecar_resources_into_app` function (imperative `cp`-based, matching the existing frontend-dist copy at `package_dmg.sh:60-64` and the helper-bundler at `_bundle_helpers.sh:22-39`). The Node binaries for `aarch64` and `x86_64` are downloaded at packaging time from nodejs.org into a staging dir, then copied alongside `pi-sidecar.bundle.js` and a build-time-generated `manifest.json` into `Contents/Resources/pi-sidecar/`. The nested `node` binaries are signed with a dedicated `sidecar-node.plist` entitlements file (`allow-jit` only, per spec §378) BEFORE the outer `.app` re-seal. Runtime path resolution reuses the existing `SubagentPiSidecarConfig.runtime_dir` field + `BusytokPaths::sidecar_runtime_dir()` (no new locator module). The GUI Tauri setup hook resolves the packaged sidecar dir via a `current_exe()` walk-up, then refreshes the locator via the new service-owned `pi_sidecar_locator_update` RPC — which atomically updates the in-memory `Arc<Mutex<BusytokSettings>>` AND persists to `settings.toml` (mirrors `provider_update`). A direct file write is used ONLY as a cold-start fallback when the service is transport-unreachable (socket/bootstrap failure); service business errors are logged + surfaced, NOT bypassed. No `current_exe()` path guessing in the service (spec §373).

**Tech Stack:** Bash (packaging), Rust (config schema + GUI Tauri hook + existing doctor checks), JSON (`manifest.json`), Apple `codesign` + entitlements plist, `curl` + `tar` (Node binary download), `grep` (PROTOCOL_VERSION extraction from Rust source), esbuild (existing CJS bundle, untouched).

## Global Constraints

[Verbatim from spec §3.4, §3.5, §5 (Phase 5), §6 (Phase 5 acceptance). Every task implicitly includes these.]

- `.app` structure (spec §351-358): `Busytok.app/Contents/Resources/pi-sidecar/{pi-sidecar.bundle.js, manifest.json, node/aarch64/node, node/x86_64/node}` — exact directory name `pi-sidecar` (NOT `sidecars/pi` despite stale docstrings at `crates/busytok-config/src/lib.rs:252-253`).
- `manifest.json` schema (spec §360-368): `{"version":"1","protocol_version":1,"bundle":"pi-sidecar.bundle.js","node_runtime_version":"22.6.0"}` — exact keys, `version` is the manifest schema version (string "1"), `protocol_version` is the integer constant from `crates/busytok-subagent/src/sidecar/protocol.rs::PROTOCOL_VERSION`, `node_runtime_version` is the Node major.minor.patch used at build time.
- Runtime path resolution (spec §370-373): packaged build uses a **persisted locator** (`runtime_dir` in `settings.toml`); the GUI injection is a **refresh mechanism**, not the sole source — the service reads the persisted locator on its own startup (including login auto-start and CLI-only invocation without GUI). **No `current_exe()` path guessing** in the service. Dev mode uses `runtime_dir` from `settings.toml` or fallback to `apps/pi-sidecar/dist/` (existing behavior).
- Signing order (spec §375-381, sequential): (1) Build `.app` via Tauri; (2) Copy sidecar resources into `Contents/Resources/pi-sidecar/`; (3) Sign nested node binaries with Developer ID + `sidecar-node.plist` entitlements (`allow-jit` only); (4) Sign outer `.app` bundle (re-seal); (5) Notarize + staple; (6) Package DMG.
- Entitlements (spec §383): applied to the `node` binary's code signature, NOT to `busytok-service`'s entitlement plist. Start with `allow-jit` only (minimal privilege). Only add `allow-unsigned-executable-memory` if notarized runtime testing proves Node 22 requires it under hardened runtime — **verify empirically before broadening**. This plan adds `allow-jit` only.
- Dual-architecture (spec §385-388): both `aarch64` and `x86_64` node binaries bundled. Runtime selects ONLY the current host architecture (via `BusytokPaths::sidecar_bundled_node_path()` which joins `std::env::consts::ARCH`). Never attempts Rosetta cross-architecture fallback.
- DMG size (spec §390): ~95MB increase (15MB bundle + 40MB × 2 node binaries). Current ~30MB → ~125MB. Acceptance: DMG size < 150MB.
- Doctor checks (spec §392-409): the existing `run_subagent_doctor` emits 11 checks (the spec table lists 10 — the "11th" `subagents_unused_30d` is a pre-existing warning-only check, not added by Phase 5). After Phase 5, all checks that currently fail in a packaged build (`bundled_node_arch`, `bundle_manifest_readable`, `pi_runtime_installed`) move to `ok`. The `protocol_version` check (currently "warning" because `pi_sidecar.enabled=false` by default) moves to `ok` when the packaged `settings.toml` sets `pi_sidecar.enabled=true`. **No new doctor check is added.**
- `release_script_smoke.sh` verify checks (spec §411-414, new): `pi-sidecar.bundle.js` exists; `manifest.json` exists + valid JSON; node binary exists + executable for current arch.
- Deferred (spec §416-420): Node SEA (Single Executable Application); per-architecture DMG distribution; Node version auto-update; CI-level end-to-end delegate test on packaged build.
- `provider.id` immutable after creation (spec §3.4); `provider.models` is a whitelist (spec §3.4); built-in profiles canonicalized on load (spec §3.4). [Pre-existing, unaffected by Phase 5.]
- Phase 5 acceptance (spec §472-477): DMG includes bundle + manifest + node binaries; fresh install `busytok doctor` passes all checks; fresh install `busytok delegate` spawns sidecar and returns model output; node binary signed with JIT entitlements; runtime selects correct arch node (no Rosetta fallback); DMG size < 150MB.

---

## File Structure

**Files created:**
- `packaging/macos/scripts/_bundle_sidecar.sh` — new shell helper: downloads Node binaries (both arches), generates `manifest.json`, copies bundle + manifest + node into `Contents/Resources/pi-sidecar/`. Sourced by `package_dmg.sh`.
- `packaging/macos/entitlements/sidecar-node.plist` — new entitlements plist for the nested Node binary (`allow-jit` only, per spec §383).
- `crates/busytok-config/src/manifest.rs` — new Rust module: typed `SidecarManifest` struct + `serde` impl + `to_json_string()` writer. Used by the doctor check (replaces opaque `serde_json::Value` parse) and exposed for test reuse.
- `crates/busytok-runtime/tests/sidecar_bundling_doctor.rs` — new integration test: stages a complete packaged sidecar dir layout and verifies the 3 previously-failing doctor checks now pass.

**Files modified:**
- `packaging/macos/scripts/package_dmg.sh` — insert sidecar bundling step (source `_bundle_sidecar.sh`, call `bundle_sidecar_resources_into_app`, sign node binaries with `sidecar-node.plist`) between the existing helper-bundling step (line 73) and the `.app` re-seal (line 117).
- `packaging/macos/scripts/_release_vars.sh` — add `SIDECAR_NODE_VERSION` variable.
- `tests/packaging/macos/release_script_smoke.sh` — add 6 contract checks (bundle source call, manifest generator, dual-arch download, entitlements present + allow-jit-only, node signing).
- `crates/busytok-config/src/lib.rs` — re-export `SidecarManifest` from the new `manifest` module; fix the stale docstrings at lines 252-253 (`sidecars/pi` → `pi-sidecar` in both the packaged-GUI and service-only example paths).
- `crates/busytok-runtime/src/supervisor.rs` — the `bundle_manifest_readable` doctor check (lines 938-974) switches from opaque `serde_json::Value` to typed `SidecarManifest` deserialize (validates schema fields: `version`, `protocol_version`, `bundle`, `node_runtime_version`). Add `tracing::warn!` on manifest read/parse failure with `event_code = "subagent.doctor.manifest_invalid"`.
- `apps/gui/src-tauri/src/lib.rs` — in the Tauri `setup` hook (around line 340, after `paths_for_lc`), resolve the packaged sidecar dir via `current_exe()` walk-up, then refresh the locator via the service-owned `pi_sidecar_locator_update` RPC (updates in-memory + on-disk atomically). Falls back to direct file write if service is unreachable (cold-start). Add `resolve_sidecar_dir_from_exe` + `resolve_packaged_sidecar_dir` + `refresh_sidecar_locator` functions. Guard: only in packaged mode (`.app` ancestor found + `Resources/pi-sidecar/` exists), never in dev.
- `crates/busytok-control/src/dispatch.rs` — add `pi_sidecar_locator_update` to the `RuntimeControl` trait, the dispatch match case (`"pi_sidecar_locator_update"`), and a `TestRuntimeControl` stub.
- `crates/busytok-protocol/src/dto.rs` — add `PiSidecarLocatorUpdateRequestDto` + `PiSidecarLocatorUpdateResponseDto` (narrow DTOs for the service-owned locator update).
- `crates/busytok-runtime/src/supervisor.rs` — add `pi_sidecar_locator_update` method (mirrors `provider_update`: clone → mutate → save → swap in-memory) + `#[doc(hidden)] pub fn pi_sidecar_state` test accessor. The existing `bundle_manifest_readable` doctor check (lines 938-974) switches from opaque `serde_json::Value` to typed `SidecarManifest` deserialize.

**Files NOT modified (verified correct as-is):**
- `crates/busytok-config/src/paths.rs` — `sidecar_runtime_dir()`, `sidecar_bundle_path()`, `sidecar_manifest_path()`, `sidecar_bundled_node_path()` are all correct and used as-is.
- `crates/busytok-subagent/src/sidecar/config.rs` — `resolve_base_sidecar_config()` already does explicit `bundled`/`system` selection with no silent fallback.
- `crates/busytok-runtime/src/supervisor.rs` doctor checks #4 (`bundled_node_arch`), #8 (`pi_runtime_installed`) — logic is correct; they pass once resources are present.
- `apps/pi-sidecar/esbuild.config.mjs` — CJS bundle output unchanged.
- `.github/workflows/release.yml` — calls `package_dmg.sh`; no parallel packaging logic.

---

## Task 1: Sidecar Manifest Module (Rust typed struct + doctor check upgrade)

**Files:**
- Create: `crates/busytok-config/src/manifest.rs`
- Modify: `crates/busytok-config/src/lib.rs` (add `mod manifest;` + re-export, fix stale docstring)
- Modify: `crates/busytok-runtime/src/supervisor.rs:938-974` (typed deserialize + tracing)
- Test: `crates/busytok-config/src/manifest.rs` (unit tests inline)
- Test: `crates/busytok-runtime/tests/subagent_e2e_sidecar.rs` (existing doctor tests, verify they still pass)

**Interfaces:**
- Produces: `pub struct SidecarManifest { pub version: String, pub protocol_version: u32, pub bundle: String, pub node_runtime_version: String }` with `impl SidecarManifest { pub fn to_json_string(&self) -> String }` and `impl<'de> Deserialize<'de>`.
- Consumes: `busytok_subagent::sidecar::protocol::PROTOCOL_VERSION` (u32 constant at `crates/busytok-subagent/src/sidecar/protocol.rs:28`).

**Why this task is first:** The manifest schema is the contract that both the build-time generator (Task 2) and the runtime doctor check (this task) depend on. Defining the typed struct first prevents drift between what the shell generates and what Rust validates.

- [ ] **Step 1: Write the failing test for the manifest struct**

Create `crates/busytok-config/src/manifest.rs` with the struct + tests:

```rust
//! Typed schema for the pi-sidecar `manifest.json` (spec §360-368).
//!
//! Generated at build time by `packaging/macos/scripts/_bundle_sidecar.sh`
//! and read by the `bundle_manifest_readable` doctor check. This struct is
//! the single source of truth for the manifest schema — the shell generator
//! and the Rust validator both reference these field names.

use serde::{Deserialize, Serialize};

/// Manifest schema for `Contents/Resources/pi-sidecar/manifest.json`.
///
/// Schema version is `"1"` (string, not integer) per spec §362. All fields are
/// required — a manifest missing any field is invalid and must cause the
/// `bundle_manifest_readable` doctor check to return `"error"`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SidecarManifest {
    /// Manifest schema version. Currently `"1"`.
    pub version: String,
    /// Sidecar protocol version (matches `PROTOCOL_VERSION` in
    /// `crates/busytok-subagent/src/sidecar/protocol.rs`). Serialized as a
    /// JSON integer per spec §363. Type is `u32` to match the constant's
    /// actual type (NOT i64 — direct assignment without a cast).
    pub protocol_version: u32,
    /// Filename of the JS bundle within the same directory.
    /// Always `"pi-sidecar.bundle.js"`.
    pub bundle: String,
    /// Node runtime version string (e.g. `"22.6.0"`).
    pub node_runtime_version: String,
}

impl SidecarManifest {
    /// Serialize to a pretty-printed JSON string for build-time generation.
    pub fn to_json_string(&self) -> String {
        serde_json::to_string_pretty(self)
            .expect("SidecarManifest serialization is infallible")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_round_trips_through_json() {
        let manifest = SidecarManifest {
            version: "1".to_string(),
            protocol_version: 1,
            bundle: "pi-sidecar.bundle.js".to_string(),
            node_runtime_version: "22.6.0".to_string(),
        };
        let json = manifest.to_json_string();
        let parsed: SidecarManifest = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, manifest);
    }

    #[test]
    fn manifest_deserializes_from_canonical_json() {
        let json = r#"{
            "version": "1",
            "protocol_version": 1,
            "bundle": "pi-sidecar.bundle.js",
            "node_runtime_version": "22.6.0"
        }"#;
        let parsed: SidecarManifest = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.version, "1");
        assert_eq!(parsed.protocol_version, 1);
        assert_eq!(parsed.bundle, "pi-sidecar.bundle.js");
        assert_eq!(parsed.node_runtime_version, "22.6.0");
    }

    #[test]
    fn manifest_rejects_missing_field() {
        let json = r#"{"version":"1","protocol_version":1,"bundle":"pi-sidecar.bundle.js"}"#;
        let result: Result<SidecarManifest, _> = serde_json::from_str(json);
        assert!(result.is_err(), "missing node_runtime_version must error");
    }

    #[test]
    fn manifest_rejects_wrong_type_for_protocol_version() {
        // protocol_version must be an integer, not a string.
        let json = r#"{"version":"1","protocol_version":"1","bundle":"pi-sidecar.bundle.js","node_runtime_version":"22.6.0"}"#;
        let result: Result<SidecarManifest, _> = serde_json::from_str(json);
        assert!(result.is_err(), "string protocol_version must error");
    }

    #[test]
    fn manifest_to_json_string_is_pretty() {
        let manifest = SidecarManifest {
            version: "1".to_string(),
            protocol_version: 1,
            bundle: "pi-sidecar.bundle.js".to_string(),
            node_runtime_version: "22.6.0".to_string(),
        };
        let json = manifest.to_json_string();
        assert!(json.contains('\n'), "pretty-printed JSON has newlines");
        assert!(json.contains("\"protocol_version\": 1"));
    }
}
```

- [ ] **Step 2: Register the module + fix stale docstring**

In `crates/busytok-config/src/lib.rs`:
- Add `mod manifest;` near the other `mod` declarations.
- Add `pub use manifest::SidecarManifest;` to the re-exports.
- Fix the stale docstring paths at lines 252-253: change `Resources/sidecars/pi` → `Resources/pi-sidecar` AND `lib/busytok/sidecars/pi` → `lib/busytok/pi-sidecar` to match spec §353.

Locate the docstring block on `SubagentPiSidecarConfig.runtime_dir` (around line 245-256). Two example lines read:
```
///   - Packaged GUI (macOS): `/Applications/Busytok.app/Contents/Resources/sidecars/pi`
///   - Service-only: `/usr/local/lib/busytok/sidecars/pi` (or wherever the
```
Change both to use the exact `pi-sidecar` directory name:
```
///   - Packaged GUI (macOS): `/Applications/Busytok.app/Contents/Resources/pi-sidecar`
///   - Service-only: `/usr/local/lib/busytok/pi-sidecar` (or wherever the
```

Then verify no other stale `sidecars/pi` references exist in the codebase:

```bash
grep -rn 'sidecars/pi' crates/ apps/ packaging/
```

Expected: no output. If any matches appear (other docstrings, comments, or test fixtures), fix them to use `pi-sidecar` as well. If the grep returns results, add them to the commit in Step 8.

- [ ] **Step 3: Run the new tests to verify they pass**

Run: `cargo test -p busytok-config manifest`
Expected: 5 tests pass.

- [ ] **Step 4: Upgrade the doctor check to use typed deserialize**

In `crates/busytok-runtime/src/supervisor.rs`, the `bundle_manifest_readable` check (lines 938-974) currently parses into `serde_json::Value`. Replace the parse with `SidecarManifest`:

```rust
// 5. Bundle manifest readable (spec §7.1 line 866, §5.1 line 549).
//    Verifies manifest.json EXISTS, is READABLE, is PARSEABLE JSON, AND
//    conforms to the SidecarManifest schema (version/protocol_version/
//    bundle/node_runtime_version all present + correct types). A missing
//    or malformed manifest is an "error" — the sidecar cannot be launched
//    without a valid manifest.
{
    let manifest_path = self.paths.sidecar_manifest_path(runtime_dir_ref);
    let (status, detail) = match std::fs::read_to_string(&manifest_path) {
        Ok(contents) => match serde_json::from_str::<busytok_config::SidecarManifest>(&contents) {
            Ok(m) => (
                "ok",
                format!(
                    "manifest readable (version={}, protocol_version={}, node={}): {}",
                    m.version, m.protocol_version, m.node_runtime_version,
                    manifest_path.display()
                ),
            ),
            Err(e) => {
                tracing::warn!(
                    event_code = "subagent.doctor.manifest_invalid",
                    path = %manifest_path.display(),
                    error = %e,
                    "manifest at path is not a valid SidecarManifest"
                );
                (
                    "error",
                    format!(
                        "manifest at {} is not a valid SidecarManifest: {}",
                        manifest_path.display(),
                        e
                    ),
                )
            }
        },
        Err(e) => {
            tracing::warn!(
                event_code = "subagent.doctor.manifest_unreadable",
                path = %manifest_path.display(),
                error = %e,
                "manifest not readable at path"
            );
            (
                "error",
                format!(
                    "manifest not readable at {}: {}",
                    manifest_path.display(),
                    e
                ),
            )
        },
    };
    checks.push(DoctorCheckDto {
        name: "bundle_manifest_readable".into(),
        status: status.into(),
        detail: Some(detail),
    });
}
```

- [ ] **Step 5: Update the existing test that writes a non-conforming manifest**

The existing test `doctor_bundle_manifest_readable_check_validates_manifest` in `crates/busytok-runtime/tests/subagent_e2e_sidecar.rs:913-944` writes `{"name":"pi-sidecar","version":"0.1.0"}` as the manifest content. This JSON is valid but does NOT conform to the `SidecarManifest` schema (missing `protocol_version`, `bundle`, `node_runtime_version`; has extra fields `name`, `version`). After Step 4's switch to typed `serde_json::from_str::<SidecarManifest>`, deserialization fails and the test's `assert_eq!(check.status, "ok", ...)` assertion breaks.

Update the manifest content to conform to the schema. In `crates/busytok-runtime/tests/subagent_e2e_sidecar.rs`, find the `doctor_bundle_manifest_readable_check_validates_manifest` test (around line 918-922) and replace the raw JSON bytes:

```rust
    // Write a valid manifest.json conforming to SidecarManifest schema
    // (Task 1: version/protocol_version/bundle/node_runtime_version).
    std::fs::write(
        runtime_dir.join("manifest.json"),
        br#"{"version":"1","protocol_version":1,"bundle":"pi-sidecar.bundle.js","node_runtime_version":"22.6.0"}"#,
    )
    .unwrap();
```

The malformed-manifest test (`doctor_bundle_manifest_readable_check_fails_on_malformed_manifest` at line 948) writes `"not json {{{"` which still fails `serde_json::from_str::<SidecarManifest>` — no change needed there.

- [ ] **Step 6: Run the existing doctor tests to verify no regression**

Run: `cargo test -p busytok-runtime --test subagent_e2e_sidecar doctor_bundle_manifest`
Expected: 2 tests pass (`_validates_manifest` + `_fails_on_malformed_manifest`). The `_validates_manifest` test now writes a schema-conforming manifest (Step 5); the `_fails_on_malformed_manifest` test writes `"not json {{{"` which still fails `serde_json::from_str::<SidecarManifest>` — same behavior, just typed.

- [ ] **Step 7: Run the full workspace test suite**

Run: `cargo test --workspace`
Expected: all pass (0 new failures).

- [ ] **Step 8: Commit**

```bash
git add crates/busytok-config/src/manifest.rs crates/busytok-config/src/lib.rs crates/busytok-runtime/src/supervisor.rs crates/busytok-runtime/tests/subagent_e2e_sidecar.rs
git commit -m "feat(config): add typed SidecarManifest + upgrade doctor check to schema-aware parse"
```

---

## Task 2: Build-Time Sidecar Bundling Script (manifest generation + Node download + resource copy)

**Files:**
- Create: `packaging/macos/scripts/_bundle_sidecar.sh`
- Create: `packaging/macos/entitlements/sidecar-node.plist`
- Modify: `packaging/macos/scripts/_release_vars.sh` (add `SIDECAR_NODE_VERSION` + staging dir)
- Modify: `packaging/macos/scripts/package_dmg.sh` (source + invoke the new helper, sign node binaries)
- Test: `tests/packaging/macos/release_script_smoke.sh` (verify checks added in Task 5, not here)

**Interfaces:**
- Consumes: `apps/pi-sidecar/dist/pi-sidecar.bundle.js` (existing esbuild output), `crates/busytok-subagent/src/sidecar/protocol.rs::PROTOCOL_VERSION` (read via `cargo run` or hardcoded + asserted).
- Produces: `<APP_PATH>/Contents/Resources/pi-sidecar/{pi-sidecar.bundle.js, manifest.json, node/aarch64/node, node/x86_64/node}`.

**Why this order:** Task 1 defines the manifest schema; Task 2 generates a manifest conforming to it. Task 3 (GUI persistence) depends on the resources being present in the `.app`. Task 5 (smoke test) depends on Task 2's output contract.

- [ ] **Step 1: Add release vars**

In `packaging/macos/scripts/_release_vars.sh`, append:

```bash
# Phase 5: Node runtime version bundled into the .app for the pi-sidecar.
# Pinned to a specific major.minor.patch from nodejs.org. Bump explicitly;
# there is no auto-update (spec §419 deferred).
SIDECAR_NODE_VERSION="22.6.0"
```

Read the current `_release_vars.sh` first to match its style (it likely defines `BUNDLE_ROOT`, `BUNDLE_MACOS_DIR`, etc.). Append the new vars at the end.

- [ ] **Step 2: Create the sidecar-node entitlements plist**

Create `packaging/macos/entitlements/sidecar-node.plist`:

```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>com.apple.security.cs.allow-jit</key>
    <true/>
    <key>com.apple.security.app-sandbox</key>
    <false/>
</dict>
</plist>
```

Per spec §383: start with `allow-jit` only. `app-sandbox=false` matches the existing `service.plist`/`app.plist` pattern (the app is not sandboxed). Do NOT add `allow-unsigned-executable-memory` until notarized runtime testing proves Node 22 requires it — verify empirically before broadening.

- [ ] **Step 3: Create the bundling helper script**

Create `packaging/macos/scripts/_bundle_sidecar.sh`:

```bash
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

    mkdir -p "$dest_dir" "$staging/cache"

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
    ( cd "$PROJECT_ROOT/apps/pi-sidecar" && pnpm run build )
    if [ ! -f "$bundle_src" ]; then
        echo "  ERROR: pi-sidecar.bundle.js not produced by build"
        return 1
    fi

    mkdir -p "$sidecar_dir/node/aarch64" "$sidecar_dir/node/x86_64"

    # 2. Copy the JS bundle.
    cp "$bundle_src" "$sidecar_dir/pi-sidecar.bundle.js" || { echo "  ERROR: cp bundle failed"; return 1; }
    echo "  bundled pi-sidecar.bundle.js"

    # 3. Generate manifest.json (Task 1 schema).
    generate_sidecar_manifest "$sidecar_dir" || return 1

    # 4. Download + extract both Node arches into staging, then copy.
    # Iterate NODE_ARCHES: each entry is "<upstream_token> <app_dir>".
    local staging
    staging="$(mktemp -d "${TMPDIR:-/tmp}/busytok-node-staging.XXXXXX")"
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
```

- [ ] **Step 4: Wire the helper into package_dmg.sh**

In `packaging/macos/scripts/package_dmg.sh`, after the existing `bundle_helpers_into_app` + `bundle_service_plist_into_app` calls (line 73) and BEFORE the `if [ -n "${DEVELOPER_ID_APPLICATION:-}" ]` signing block (line 83), insert:

```bash
    # Phase 5: bundle pi-sidecar resources (JS bundle + manifest + dual-arch
    # Node binaries) into Contents/Resources/pi-sidecar/ (spec §345-390).
    source "$SCRIPT_DIR/_bundle_sidecar.sh"
    bundle_sidecar_resources_into_app "$APP_PATH" || {
        echo "  ERROR: sidecar bundling failed"
        exit 1
    }
```

Then, inside the existing `if [ -n "${DEVELOPER_ID_APPLICATION:-}" ]` block, AFTER the helper-binary signing loop (line 111, after the `done` that signs `busytok-service` + `busytok`) and BEFORE the `# Re-seal the app bundle` comment (line 113), insert the node-binary signing call:

```bash
        # Phase 5: sign the nested node binaries with allow-jit entitlements
        # BEFORE the outer bundle re-seal (spec §375-381 signing order).
        sign_sidecar_node_binaries "$APP_PATH" "$DEVELOPER_ID_APPLICATION" "$TIMESTAMP_FLAG"
```

This places node-binary signing between helper signing and the `.app` re-seal, exactly matching the spec's sequential signing order.

- [ ] **Step 5: Make the script executable + verify syntax**

The smoke test in Task 4 checks `[ -x "$HELPER" ]`. The `Write` tool creates files without execute permission, so chmod is required:

```bash
chmod +x packaging/macos/scripts/_bundle_sidecar.sh
bash -n packaging/macos/scripts/_bundle_sidecar.sh && bash -n packaging/macos/scripts/package_dmg.sh
```
Expected: no output (syntax OK). The existing `_bundle_helpers.sh` is already executable — this matches the convention.

- [ ] **Step 6: Verify the manifest generator produces valid JSON**

Run a focused test of just the manifest generator:
```bash
source packaging/macos/scripts/_release_vars.sh
source packaging/macos/scripts/_bundle_helpers.sh
source packaging/macos/scripts/_bundle_sidecar.sh
tmp=$(mktemp -d)
generate_sidecar_manifest "$tmp"
cat "$tmp/manifest.json"
# Validate it parses as the Rust SidecarManifest struct:
cargo test -p busytok-config manifest -- --nocapture
```
Expected: manifest.json printed with `protocol_version` matching the constant in `protocol.rs`, and the Rust tests pass.

- [ ] **Step 7: Commit**

```bash
git add packaging/macos/scripts/_bundle_sidecar.sh packaging/macos/entitlements/sidecar-node.plist packaging/macos/scripts/_release_vars.sh packaging/macos/scripts/package_dmg.sh
git commit -m "feat(packaging): add pi-sidecar bundling script + node binary signing"
```

---

## Task 3: GUI Startup Runtime-Dir Persistence + pi_sidecar Enablement (service-owned update, file fallback)

**Files:**
- Modify: `crates/busytok-runtime/src/supervisor.rs` (add `pi_sidecar_locator_update` method — service-owned in-memory + disk update, mirrors `provider_update` pattern at line 5398; add `#[doc(hidden)] pub fn pi_sidecar_state` test accessor next to `with_adapters_and_settings` at line 164)
- Modify: `crates/busytok-control/src/dispatch.rs` (register the new method in the `RuntimeControl` dispatch — add `PiSidecarLocatorUpdate` variant)
- Modify: `crates/busytok-protocol/src/dto.rs` (add `PiSidecarLocatorUpdateRequestDto` — narrow DTO for just `runtime_dir` + `enabled`)
- Modify: `apps/gui/src-tauri/src/lib.rs` (Tauri setup hook, after line 340 `paths_for_lc`; add `resolve_sidecar_dir_from_exe` + `resolve_packaged_sidecar_dir` + `refresh_sidecar_locator` functions; declare `#[cfg(test)] mod phase5_tests;`)
- Create: `apps/gui/src-tauri/src/phase5_tests.rs` (test module — file-module form per existing convention)
- Test: `crates/busytok-runtime/tests/subagent_e2e_sidecar.rs` (service-owned update integration test)
- Test: `apps/gui/src-tauri/src/phase5_tests.rs` (GUI path resolver unit tests)

**Interfaces:**
- Consumes: `busytok_config::BusytokPaths`, `busytok_config::BusytokSettings` (load/save), `std::env::current_exe()`, the existing `invoke_busytok_via_socket_with_bootstrap` RPC path, `tracing::warn!`/`info!` with `event_code`.
- Produces: a service-owned update that atomically (1) mutates the in-memory `Arc<Mutex<BusytokSettings>>` AND (2) persists to `settings.toml`. GUI calls this via the existing `invoke_busytok` RPC. File fallback only when service is unreachable.

**Decision: two-phase update (service-owned RPC primary, file fallback).** The spec (§371) says the GUI injection is a "refresh mechanism" — but the daemon holds settings in shared memory (`Arc<Mutex<BusytokSettings>>` at supervisor.rs:334), NOT re-read from disk on each use. A direct file write would leave the running daemon's in-memory state stale: "file fixed, current session still can't find sidecar" — a state-drift bug (P1 review finding).

The correct approach mirrors the existing `provider_update` pattern (supervisor.rs:5398-5436): clone → mutate → save_to_file → swap in-memory. We add a narrow service method `pi_sidecar_locator_update(runtime_dir, enabled)` that does exactly this. The GUI calls it via the existing `invoke_busytok` RPC.

For the cold-start edge case (GUI starts before the service on first login auto-start), the GUI also writes directly to `settings.toml` as a fallback so the service reads the locator on its own startup. The service-owned RPC is the primary path (keeps in-memory state fresh); the file fallback only fires when the RPC fails.

**Decision: current_exe() walk-up, not Tauri resource_dir() API.** Spec §371 mentions "via Tauri resource API". This plan deliberately deviates: `current_exe()` walk-up is simpler, testable without an `AppHandle`, and resolves to the same `Contents/Resources/` path. The Tauri API would add a dependency on `AppHandle` in the resolver, making it untestable in isolation. The deviation is documented in the `resolve_packaged_sidecar_dir` docstring.

**Why path-resolution + service-update + fallback are one task:** All three are triggered by the same packaged-mode detection on GUI startup. Splitting them would duplicate the `.app` walk-up. Keeping them together is DRY and atomic.

**Testability design (critical):** The path-resolution core is extracted into a testable free function that takes an explicit parameter (no `current_exe()` dependency):
- `resolve_sidecar_dir_from_exe(exe: &Path) -> Option<PathBuf>` — pure function of a path + filesystem checks.

Tests exercise the real free function directly — NOT a mirror — so modifying production code without updating tests will correctly fail. The service-owned update method is tested via an integration test that calls `pi_sidecar_locator_update` and verifies both the in-memory state AND the persisted file.

- [ ] **Step 1: Add the DTO + trait method + dispatch case + TestRuntimeControl stub**

This step adds the scaffolding so Step 2's tests can compile against the trait. 4 edits across 2 files:

In `crates/busytok-protocol/src/dto.rs`, add the DTOs:

```rust
/// Request to update the pi_sidecar locator fields (runtime_dir + enabled).
/// Spec §371: GUI injects the packaged sidecar path on startup; the service
/// owns the in-memory + on-disk mutation (mirrors provider_update pattern).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PiSidecarLocatorUpdateRequestDto {
    /// Absolute path to the sidecar resource directory
    /// (e.g. `/Applications/Busytok.app/Contents/Resources/pi-sidecar`).
    pub runtime_dir: String,
    /// Whether the pi_sidecar subsystem should be enabled. In packaged
    /// mode, this is `true` so the protocol_version doctor check moves
    /// from "warning" to "ok" (spec §406).
    pub enabled: bool,
}

/// Response confirming the persisted locator state.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PiSidecarLocatorUpdateResponseDto {
    pub runtime_dir: String,
    pub enabled: bool,
    /// `true` if the in-memory settings were also updated (service was
    /// reachable); `false` if only the file was written (fallback path).
    pub in_memory_updated: bool,
}
```

In `crates/busytok-control/src/dispatch.rs`:

**(a) Add the trait method** (in the `RuntimeControl` trait, near `provider_update` around line ~180):

```rust
    // Phase 5: pi_sidecar locator (service-owned in-memory + on-disk update)
    async fn pi_sidecar_locator_update(
        &self,
        req: PiSidecarLocatorUpdateRequestDto,
    ) -> Result<PiSidecarLocatorUpdateResponseDto>;
```

**(b) Add the dispatch case** (in the `ControlDispatcher::dispatch` match, near `"provider.update"` at line 535):

```rust
            "pi_sidecar_locator_update" => {
                let req: PiSidecarLocatorUpdateRequestDto = serde_json::from_value(request.params)
                    .map_err(|e| anyhow::anyhow!("invalid params for pi_sidecar_locator_update: {e}"))?;
                let dto = self.runtime.pi_sidecar_locator_update(req).await?;
                ControlResponse::ok(serde_json::to_value(dto)?)
            }
```

**(c) Add a stub to `TestRuntimeControl`** (near line 638, in the `impl RuntimeControl for TestRuntimeControl` block). This keeps the test double compiling:

```rust
    async fn pi_sidecar_locator_update(
        &self,
        req: PiSidecarLocatorUpdateRequestDto,
    ) -> Result<PiSidecarLocatorUpdateResponseDto> {
        Ok(PiSidecarLocatorUpdateResponseDto {
            runtime_dir: req.runtime_dir,
            enabled: req.enabled,
            in_memory_updated: true,
        })
    }
```

- [ ] **Step 2: Write the GUI path resolver tests (TDD red phase)**

Create `apps/gui/src-tauri/src/phase5_tests.rs`:

```rust
#![allow(clippy::unwrap_used, clippy::uninlined_format_args)]
//! Phase 5 tests: path resolver (the GUI-side pure function).
//!
//! Tests the REAL production free function (resolve_sidecar_dir_from_exe),
//! NOT a mirror — so modifying production code without updating tests will
//! correctly fail. The service-owned update path is tested in
//! subagent_e2e_sidecar.rs (integration test).

use super::*;
use std::fs;
use tempfile::TempDir;

#[test]
fn resolves_sidecar_dir_when_app_bundle_contains_resources() {
    let tmp = TempDir::new().unwrap();
    let app_root = tmp.path().join("Busytok.app");
    let sidecar = app_root.join("Contents/Resources/pi-sidecar");
    fs::create_dir_all(&sidecar).unwrap();
    fs::write(sidecar.join("pi-sidecar.bundle.js"), "// stub").unwrap();
    let exe = app_root.join("Contents/MacOS/Busytok");
    fs::create_dir_all(exe.parent().unwrap()).unwrap();
    fs::write(&exe, b"stub").unwrap();

    // Call the REAL production function.
    let resolved = resolve_sidecar_dir_from_exe(&exe);
    assert_eq!(resolved, Some(sidecar));
}

#[test]
fn returns_none_when_no_app_bundle_ancestor() {
    let tmp = TempDir::new().unwrap();
    let exe = tmp.path().join("busytok-gui");
    fs::write(&exe, b"stub").unwrap();
    let resolved = resolve_sidecar_dir_from_exe(&exe);
    assert_eq!(resolved, None);
}

#[test]
fn returns_none_when_app_bundle_has_no_sidecar_resources() {
    let tmp = TempDir::new().unwrap();
    let app_root = tmp.path().join("Busytok.app");
    fs::create_dir_all(app_root.join("Contents/MacOS")).unwrap();
    // No Contents/Resources/pi-sidecar directory.
    let exe = app_root.join("Contents/MacOS/Busytok");
    fs::write(&exe, b"stub").unwrap();
    let resolved = resolve_sidecar_dir_from_exe(&exe);
    assert_eq!(resolved, None);
}

#[test]
fn classifies_transport_vs_business_errors() {
    // P1 guard: the file fallback must fire ONLY on transport-unreachable
    // errors (cold-start), NOT on service business errors. A business error
    // indicates a real bug; bypassing it with a file write would mask the
    // bug and re-introduce in-memory/disk state drift.

    // Transport-unreachable → file fallback (true).
    assert!(is_transport_unreachable("connect/bootstrap phase timed out"));
    assert!(is_transport_unreachable("service unavailable: connection refused"));
    assert!(is_transport_unreachable("service bootstrap failed: launchctl timeout"));
    assert!(is_transport_unreachable("call to 'pi_sidecar_locator_update' timed out"));

    // Service business errors → log + surface (false, NO fallback).
    assert!(!is_transport_unreachable("[validation_error] runtime_dir must be absolute"));
    assert!(!is_transport_unreachable("[internal_error] failed to serialize response"));
    assert!(!is_transport_unreachable("dispatch error: method not found"));
    // Edge: empty string is NOT transport-unreachable.
    assert!(!is_transport_unreachable(""));
}
```

In `apps/gui/src-tauri/src/lib.rs`, add the module declaration near the other `#[cfg(test)] mod *_tests;` lines (around lines 33-61):

```rust
#[cfg(test)]
mod phase5_tests;
```

- [ ] **Step 3: Write the service-owned update integration test (TDD red phase)**

In `crates/busytok-runtime/tests/subagent_e2e_sidecar.rs`, add a test that verifies the service-owned update mutates BOTH in-memory state AND the persisted file (the P1 state-drift regression guard):

```rust
#[tokio::test]
#[serial]
async fn pi_sidecar_locator_update_mutates_in_memory_and_disk() {
    // P1 regression guard: the service-owned update must mutate the
    // in-memory Arc<Mutex<BusytokSettings>> (so the running daemon's
    // worker pool sees the new locator immediately) AND persist to
    // settings.toml (so a cold-start service reads it on its own startup).
    // A direct file write would leave the in-memory state stale.
    let tmp = tempfile::tempdir().unwrap();
    let db = busytok_store::Database::open_in_memory().unwrap();
    let paths = BusytokPaths::for_test(tmp.path());
    let settings = BusytokSettings::default();
    settings
        .save_to_file(&paths.config_dir().join("settings.toml"))
        .unwrap();

    let supervisor =
        BusytokSupervisor::with_adapters_and_settings(db, paths.clone(), vec![], settings);

    // Pre-condition: read the in-memory state via the test accessor
    // (the `settings` field is private; `settings_snapshot` RPC returns a
    // DTO that omits pi_sidecar.runtime_dir).
    let (pre_dir, pre_enabled) = supervisor.pi_sidecar_state();
    assert!(pre_dir.is_none());
    assert!(!pre_enabled);

    // Call the service-owned update (the method the GUI invokes via RPC).
    let fake_dir = tmp.path().join("fake-pi-sidecar");
    std::fs::create_dir_all(&fake_dir).unwrap();
    let resp = supervisor
        .pi_sidecar_locator_update(PiSidecarLocatorUpdateRequestDto {
            runtime_dir: fake_dir.to_string_lossy().to_string(),
            enabled: true,
        })
        .await
        .unwrap();

    assert!(resp.in_memory_updated);
    assert_eq!(resp.runtime_dir, fake_dir.to_string_lossy());
    assert!(resp.enabled);

    // Verify in-memory state was updated (the P1 guard — no state drift).
    // pi_sidecar_state reads from the SAME Arc<Mutex<BusytokSettings>> that
    // pi_sidecar_locator_update swapped — if the swap didn't happen, this
    // assertion fails.
    let (post_dir, post_enabled) = supervisor.pi_sidecar_state();
    assert_eq!(post_dir.as_deref(), Some(fake_dir.to_str().unwrap()));
    assert!(post_enabled);

    // Verify the file was also persisted (cold-start path).
    let reloaded = BusytokSettings::load(&paths).unwrap();
    assert_eq!(
        reloaded.subagent.pi_sidecar.runtime_dir.as_deref(),
        Some(fake_dir.to_str().unwrap())
    );
    assert!(reloaded.subagent.pi_sidecar.enabled);

    supervisor.shutdown_writer().await.unwrap();
}
```

Add the import at the top of the file:

```rust
use busytok_protocol::dto::PiSidecarLocatorUpdateRequestDto;
```

- [ ] **Step 4: Run the tests to verify they FAIL (TDD red phase)**

Run both:
```bash
cargo test -p busytok-gui --lib phase5_tests
cargo test -p busytok-runtime --test subagent_e2e_sidecar pi_sidecar_locator_update_mutates_in_memory_and_disk
```
Expected: COMPILE ERROR — `resolve_sidecar_dir_from_exe` (GUI) + `pi_sidecar_locator_update` method + `pi_sidecar_state` accessor (supervisor) not yet defined. This is the TDD red phase.

- [ ] **Step 5: Implement the production code (TDD green phase)**

**(a) Add the `pi_sidecar_locator_update` method + `pi_sidecar_state` accessor on `BusytokSupervisor`** in `crates/busytok-runtime/src/supervisor.rs`. Add `pi_sidecar_locator_update` in the `impl RuntimeControl for BusytokSupervisor` block near `provider_update` (line 5398) — mirrors the clone → mutate → save → swap pattern:

```rust
    async fn pi_sidecar_locator_update(
        &self,
        req: PiSidecarLocatorUpdateRequestDto,
    ) -> Result<PiSidecarLocatorUpdateResponseDto> {
        let mut pending_settings = {
            let settings = self.settings.lock().unwrap();
            settings.clone()
        };

        let changed = pending_settings.subagent.pi_sidecar.runtime_dir.as_deref()
            != Some(req.runtime_dir.as_str())
            || pending_settings.subagent.pi_sidecar.enabled != req.enabled;

        pending_settings.subagent.pi_sidecar.runtime_dir = Some(req.runtime_dir.clone());
        pending_settings.subagent.pi_sidecar.enabled = req.enabled;

        // Persist to disk BEFORE swapping in-memory (mirrors provider_update
        // at line 5432: save first so a swap failure doesn't leave memory
        // ahead of disk).
        pending_settings.save(&self.paths)?;

        // Swap the in-memory settings so the running daemon's worker pool /
        // doctor checks see the new locator immediately.
        {
            let mut settings = self.settings.lock().unwrap();
            *settings = pending_settings;
        }

        tracing::info!(
            event_code = "pi_sidecar.locator_updated",
            runtime_dir = %req.runtime_dir,
            enabled = req.enabled,
            changed = changed,
            "pi_sidecar locator updated (in-memory + on-disk)"
        );

        Ok(PiSidecarLocatorUpdateResponseDto {
            runtime_dir: req.runtime_dir,
            enabled: req.enabled,
            in_memory_updated: true,
        })
    }
```

Add the `#[doc(hidden)]` test accessor next to `with_adapters_and_settings` (line 164) so the integration test can verify the in-memory state directly (the `settings` field is private; `settings_snapshot` RPC returns a DTO that omits `pi_sidecar.runtime_dir`):

```rust
/// Test-only accessor for the in-memory pi_sidecar locator state.
/// Used by integration tests to verify `pi_sidecar_locator_update`
/// mutated the shared Arc<Mutex<BusytokSettings>> (not just the file).
#[doc(hidden)]
pub fn pi_sidecar_state(&self) -> (Option<String>, bool) {
    let s = self.settings.lock().unwrap();
    (
        s.subagent.pi_sidecar.runtime_dir.clone(),
        s.subagent.pi_sidecar.enabled,
    )
}
```

**(b) Add the GUI path resolver free function + file fallback** in `apps/gui/src-tauri/src/lib.rs`, near the other helpers (before the `setup` closure):

```rust
// ── Phase 5: sidecar runtime-dir persistence ──────────────────────────

/// Testable core: resolve the sidecar resource directory from a given exe
/// path by walking up to find the enclosing `.app` bundle, then checking
/// for `Contents/Resources/pi-sidecar/`. Returns None if no `.app` ancestor
/// or if the sidecar directory is absent (incomplete install).
///
/// Extracted as a free function (not inlined in `resolve_packaged_sidecar_dir`)
/// so tests can exercise the real algorithm without `current_exe()`.
fn resolve_sidecar_dir_from_exe(exe: &std::path::Path) -> Option<std::path::PathBuf> {
    let mut cursor = exe.parent()?;
    loop {
        if cursor
            .file_name()
            .and_then(|n| n.to_str())
            .map(|n| n.ends_with(".app"))
            .unwrap_or(false)
        {
            let sidecar = cursor.join("Contents/Resources/pi-sidecar");
            return sidecar.is_dir().then_some(sidecar);
        }
        cursor = cursor.parent()?;
    }
}

/// Production entry point: resolve the packaged sidecar dir via
/// `current_exe()`. Deviates from spec §371 ("via Tauri resource API") —
/// `current_exe()` walk-up is simpler, testable without an `AppHandle`,
/// and resolves to the same `Contents/Resources/` path.
///
/// Spec §370-373: NO current_exe() path guessing in the SERVICE — this
/// resolution happens in the GUI only, and the result is PERSISTED to
/// settings.toml (via the service-owned RPC) so the service reads it on
/// its own startup.
fn resolve_packaged_sidecar_dir() -> Option<std::path::PathBuf> {
    let exe = std::env::current_exe().ok()?;
    resolve_sidecar_dir_from_exe(&exe)
}

/// Refresh the sidecar locator via the service-owned RPC. If the service is
/// transport-unreachable (cold-start edge case: GUI starts before service on
/// first login auto-start), fall back to a direct file write so the service
/// reads the locator on its own startup.
///
/// **Fallback condition (tightened):** The file fallback fires ONLY on
/// transport/bootstrap unreachability (socket connect failed, bootstrap
/// failed, connect/call timed out). Service business errors (validation
/// failures, serialization errors, dispatch errors) are logged + surfaced —
/// NOT bypassed — because they indicate a real bug that direct file write
/// would mask, re-introducing the in-memory/disk state drift the service-
/// owned update was designed to prevent.
///
/// Spec §371: refresh mechanism. The service owns the in-memory + on-disk
/// mutation (mirrors provider_update) — the GUI just provides the resolved
/// path. The file fallback is ONLY for the cold-start case.
async fn refresh_sidecar_locator(
    sidecar_dir: std::path::PathBuf,
    state: &tauri::State<'_, BusytokState>,
    app: &tauri::AppHandle,
) {
    let sidecar_str = sidecar_dir.to_string_lossy().to_string();

    // Primary path: service-owned RPC (updates in-memory + on-disk atomically).
    let params = serde_json::json!({
        "runtime_dir": sidecar_str,
        "enabled": true,
    });
    let rpc_result = invoke_busytok(
        "pi_sidecar_locator_update".to_string(),
        params,
        None,
        state.clone(),
        app.clone(),
    )
    .await;

    match rpc_result {
        Ok(_) => {
            tracing::info!(
                event_code = "gui.sidecar_locator_refreshed_via_rpc",
                path = %sidecar_str,
                "refreshed sidecar locator via service-owned RPC (in-memory + on-disk updated)"
            );
            return;
        }
        Err(err_str) => {
            // Distinguish transport-unreachable (cold-start → file fallback)
            // from service business errors (log + surface, do NOT bypass).
            if is_transport_unreachable(&err_str) {
                tracing::warn!(
                    event_code = "gui.sidecar_locator_rpc_unreachable_file_fallback",
                    path = %sidecar_str,
                    error = %err_str,
                    "service transport-unreachable; falling back to direct file write (cold-start case)"
                );
                // Fall through to file fallback below.
            } else {
                // Service returned a business error (validation, dispatch,
                // serialization). This is a real bug — do NOT bypass it with
                // a file write, which would mask the error and re-introduce
                // state drift. Log + surface.
                tracing::error!(
                    event_code = "gui.sidecar_locator_rpc_business_error",
                    path = %sidecar_str,
                    error = %err_str,
                    "service-owned RPC returned a business error; NOT falling back to file write"
                );
                return;
            }
        }
    }

    // File fallback: service transport-unreachable (cold-start). Write
    // directly to file so the service reads the locator on its own startup.
    // This does NOT update the running daemon's in-memory state — but the
    // daemon isn't running yet, so there's no state to drift. When the
    // service starts, it reads this file.
    let paths = busytok_config::BusytokPaths::new();
    let settings_path = paths.config_dir().join("settings.toml");
    let mut settings = busytok_config::BusytokSettings::load(&paths).unwrap_or_default();
    settings.subagent.pi_sidecar.runtime_dir = Some(sidecar_str.clone());
    settings.subagent.pi_sidecar.enabled = true;
    if let Err(e) = settings.save_to_file(&settings_path) {
        tracing::warn!(
            event_code = "gui.sidecar_settings_persist_failed",
            path = %sidecar_str,
            error = %e,
            "failed to persist sidecar settings to settings.toml (file fallback)"
        );
    }
}

/// Classify an `invoke_busytok` error string as transport-unreachable
/// (cold-start → file fallback) vs. service business error (log + surface).
///
/// Transport-unreachable errors come from:
/// - `host_application_services.rs:67` — `"connect/bootstrap phase timed out"`
/// - `service_recovery.rs:45` — `"service unavailable: {e}"`
/// - `service_recovery.rs:85` — `"service bootstrap failed: {e}"`
/// - `host_application_services.rs:74` — `"call to '{method}' timed out"`
///
/// Service business errors come from:
/// - `host_application_services.rs:80-86` — `"[{code}] {message}"` (RPC Err)
/// - `host_application_services.rs:88` — `"dispatch error: {e}"`
///
/// This classification is tested by `classifies_transport_vs_business_errors`.
fn is_transport_unreachable(err: &str) -> bool {
    err.starts_with("connect/bootstrap phase timed out")
        || err.starts_with("service unavailable:")
        || err.starts_with("service bootstrap failed:")
        || err.starts_with("call to '")
}
```

- [ ] **Step 6: Run the tests to verify they PASS (TDD green phase)**

Run both:
```bash
cargo test -p busytok-gui --lib phase5_tests
cargo test -p busytok-runtime --test subagent_e2e_sidecar pi_sidecar_locator_update_mutates_in_memory_and_disk
```
Expected: 4 GUI tests pass (3 path-resolution + 1 transport/business-error classification) + 1 integration test passes. The `in_memory_updated: true` assertion + `pi_sidecar_state()` post-condition confirm no state drift. The `classifies_transport_vs_business_errors` test confirms the file fallback fires ONLY on transport-unreachable errors, NOT on service business errors.

- [ ] **Step 7: Call the refresh function in the Tauri setup hook**

In `apps/gui/src-tauri/src/lib.rs`, inside the `setup` closure, AFTER `let paths_for_lc = busytok_config::BusytokPaths::new();` (line 340), insert (`refresh_sidecar_locator` needs the Tauri `State` + `AppHandle` for the RPC path, which are available in the setup closure):

```rust
                // Phase 5: refresh the sidecar locator via the service-owned
                // RPC (in-memory + on-disk atomic update). Falls back to a
                // direct file write if the service is unreachable (cold-start).
                // Spec §370-373, §406.
                if let Some(sidecar_dir) = resolve_packaged_sidecar_dir() {
                    refresh_sidecar_locator(sidecar_dir, &state, &app).await;
                }
```

- [ ] **Step 8: Verify the dev-mode path (no .app ancestor) does nothing**

Run the GUI in dev mode (`pnpm --filter @busytok/gui tauri dev`) and confirm `settings.toml` is NOT modified (no `runtime_dir` line added, `enabled` stays at default `false`). This is the negative path — `resolve_packaged_sidecar_dir()` returns `None`, so `refresh_sidecar_locator` is never called.

- [ ] **Step 9: Run the full workspace suite**

Run: `cargo test --workspace`
Expected: all pass (0 new failures).

- [ ] **Step 10: Commit**

```bash
git add crates/busytok-runtime/src/supervisor.rs crates/busytok-control/src/dispatch.rs crates/busytok-protocol/src/dto.rs apps/gui/src-tauri/src/lib.rs apps/gui/src-tauri/src/phase5_tests.rs crates/busytok-runtime/tests/subagent_e2e_sidecar.rs
git commit -m "feat(gui+runtime): service-owned pi_sidecar locator update (in-memory + on-disk, file fallback)"
```

---

## Task 4: Release Smoke Test Extensions (bundle contract verification)

**Files:**
- Modify: `tests/packaging/macos/release_script_smoke.sh` (add 3 verify checks per spec §411-414)

**Interfaces:**
- Consumes: the output contract of Task 2's `bundle_sidecar_resources_into_app` (the `Contents/Resources/pi-sidecar/` directory structure).

**Approach: static analysis, not output verification.** The existing `release_script_smoke.sh` greps `package_dmg.sh` for required tokens (`--app-drop-link`, forbidden tokens, etc.) — it does not execute the packaging and inspect the produced `.app`. This task follows the same convention: the 6 new checks grep `package_dmg.sh` + `_bundle_sidecar.sh` for the required contract tokens. Runtime output verification (inspecting `Contents/Resources/pi-sidecar/` of a real build) is covered by the manual smoke checklist in Task 5 Step 4, not by this static test. Keeping the convention avoids a slow `package_dmg.sh` invocation in CI.

- [ ] **Step 1: Add the verify checks to release_script_smoke.sh**

In `tests/packaging/macos/release_script_smoke.sh`, before the final `echo "=== Release script smoke PASSED ==="` (line 49), insert:

```bash
echo "  Checking pi-sidecar bundling contract in package_dmg.sh..."
PKG_DMG="$PROJECT_ROOT/packaging/macos/scripts/package_dmg.sh"
HELPER="$PROJECT_ROOT/packaging/macos/scripts/_bundle_sidecar.sh"

# Check 1: package_dmg.sh sources + calls the sidecar bundler.
if ! grep -q '_bundle_sidecar.sh' "$PKG_DMG"; then
    echo "    FAILED: package_dmg.sh does not source _bundle_sidecar.sh"
    exit 1
fi
if ! grep -q 'bundle_sidecar_resources_into_app' "$PKG_DMG"; then
    echo "    FAILED: package_dmg.sh does not call bundle_sidecar_resources_into_app"
    exit 1
fi
echo "    package_dmg.sh invokes sidecar bundler: OK"

# Check 2: the sidecar bundler script exists + is executable.
if [ ! -x "$HELPER" ]; then
    echo "    FAILED: _bundle_sidecar.sh missing or not executable"
    exit 1
fi
echo "    _bundle_sidecar.sh exists + executable: OK"

# Check 3: the bundler generates manifest.json (grep for the generator).
if ! grep -q 'generate_sidecar_manifest' "$HELPER"; then
    echo "    FAILED: _bundle_sidecar.sh does not call generate_sidecar_manifest"
    exit 1
fi
echo "    manifest.json generation: OK"

# Check 4: the bundler downloads + places node binaries for both arches.
if ! grep -q 'download_node_binary' "$HELPER"; then
    echo "    FAILED: _bundle_sidecar.sh does not call download_node_binary"
    exit 1
fi
if ! grep -q 'aarch64' "$HELPER" || ! grep -q 'x86_64' "$HELPER"; then
    echo "    FAILED: _bundle_sidecar.sh does not handle both arches"
    exit 1
fi
echo "    dual-arch node binary download: OK"

# Check 5: the sidecar-node entitlements plist exists (allow-jit only).
NODE_ENT="$PROJECT_ROOT/packaging/macos/entitlements/sidecar-node.plist"
if [ ! -f "$NODE_ENT" ]; then
    echo "    FAILED: sidecar-node.plist entitlements missing"
    exit 1
fi
if ! grep -q 'allow-jit' "$NODE_ENT"; then
    echo "    FAILED: sidecar-node.plist missing allow-jit"
    exit 1
fi
if grep -q 'allow-unsigned-executable-memory' "$NODE_ENT"; then
    echo "    FAILED: sidecar-node.plist must NOT contain allow-unsigned-executable-memory (spec §383: verify empirically before broadening)"
    exit 1
fi
echo "    sidecar-node.plist (allow-jit only): OK"

# Check 6: package_dmg.sh signs the node binaries before re-seal.
if ! grep -q 'sign_sidecar_node_binaries' "$PKG_DMG"; then
    echo "    FAILED: package_dmg.sh does not call sign_sidecar_node_binaries"
    exit 1
fi
echo "    node binary signing: OK"
```

- [ ] **Step 2: Run the smoke test**

Run: `./tests/packaging/macos/release_script_smoke.sh`
Expected: all checks pass (existing + 6 new).

- [ ] **Step 3: Commit**

```bash
git add tests/packaging/macos/release_script_smoke.sh
git commit -m "test(packaging): add pi-sidecar bundling contract checks to release smoke"
```

---

## Task 5: End-to-End Doctor Verification (integration test + manual smoke checklist)

**Files:**
- Create: `crates/busytok-runtime/tests/sidecar_bundling_doctor.rs` — integration test that stages a complete packaged sidecar dir layout and verifies the 3 previously-failing doctor checks now pass.

**Note on `subagent_e2e_sidecar.rs`:** The existing doctor tests in that file are updated in Task 1 Step 5 (manifest schema conformance). This task does NOT modify it further — the positive "all resources present" path is the new `sidecar_bundling_doctor.rs` test.

**Coverage scope (explicit):** The automated tests in this task verify 3 of the 11 doctor checks (`bundled_node_arch`, `bundle_manifest_readable`, `pi_runtime_installed`) — the ones that previously failed in dev due to missing resources. The remaining 8 checks (including `protocol_version`, which moves to `ok` only when `pi_sidecar.enabled=true` AND a real sidecar handshake succeeds) are covered by existing tests in `subagent_e2e_sidecar.rs` (negative paths) + the manual smoke checklist (positive paths). The `protocol_version` pass state requires a real Node binary + running sidecar, which CI cannot provide — it is verified by the manual smoke checklist Step 4. The >90% coverage target applies to the NEW Phase 5 code, not the entire doctor path.

**Interfaces:**
- Consumes: `busytok_config::SidecarManifest` (Task 1), `BusytokSupervisor::settings_diagnostics()` (existing), `BusytokPaths::for_test()` (existing).

- [ ] **Step 1: Write the integration test**

Create `crates/busytok-runtime/tests/sidecar_bundling_doctor.rs`:

```rust
//! Phase 5: verifies that when a complete sidecar bundle (bundle.js +
//! manifest.json + node binary) is present at the persisted runtime_dir,
//! the doctor checks `bundled_node_arch`, `bundle_manifest_readable`, and
//! `pi_runtime_installed` all return "ok".

use busytok_config::{BusytokPaths, BusytokSettings, SidecarManifest};
use busytok_runtime::BusytokSupervisor;
use busytok_store::Database;
use serial_test::serial;
use std::fs;
use tempfile::TempDir;

/// Stage a complete packaged sidecar directory: bundle.js + manifest.json +
/// node binary for the current arch. Returns the runtime_dir path.
fn stage_complete_sidecar_dir(tmp: &TempDir) -> std::path::PathBuf {
    let runtime_dir = tmp.path().join("pi-sidecar");
    let node_arch_dir = runtime_dir.join("node").join(std::env::consts::ARCH);
    fs::create_dir_all(&node_arch_dir).unwrap();

    // Write a stub bundle.js (the doctor doesn't execute it; the protocol
    // probe would, but that test lives in subagent_e2e_sidecar.rs with a
    // real mock sidecar).
    fs::write(
        runtime_dir.join("pi-sidecar.bundle.js"),
        "// stub bundle for doctor filesystem checks",
    )
    .unwrap();

    // Write a valid manifest.json conforming to SidecarManifest schema.
    let manifest = SidecarManifest {
        version: "1".to_string(),
        protocol_version: busytok_subagent::sidecar::protocol::PROTOCOL_VERSION,
        bundle: "pi-sidecar.bundle.js".to_string(),
        node_runtime_version: "22.6.0".to_string(),
    };
    fs::write(
        runtime_dir.join("manifest.json"),
        manifest.to_json_string(),
    )
    .unwrap();

    // Write a stub node binary (empty file with +x). The doctor checks
    // existence + arch-dir-name match, not that it's a real executable.
    let node_path = node_arch_dir.join("node");
    fs::write(&node_path, b"#!/bin/sh\n# stub node\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&node_path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&node_path, perms).unwrap();
    }

    runtime_dir
}

/// Build a settings config pointing at the given runtime_dir with
/// pi_sidecar DISABLED (we test filesystem checks, not the protocol probe —
/// the stub node binary can't actually run a sidecar).
fn make_settings_with_runtime_dir(runtime_dir: &str) -> BusytokSettings {
    let mut settings = BusytokSettings::default();
    settings.subagent.pi_sidecar.enabled = false;
    settings.subagent.pi_sidecar.runtime_dir = Some(runtime_dir.to_string());
    settings
}

#[tokio::test]
#[serial]
async fn doctor_passes_all_bundle_checks_when_resources_present() {
    let tmp = tempfile::tempdir().unwrap();
    let runtime_dir = stage_complete_sidecar_dir(&tmp);
    let runtime_dir_str = runtime_dir.to_string_lossy().to_string();

    let db = Database::open_in_memory().unwrap();
    let paths = BusytokPaths::for_test(tmp.path());
    let settings = make_settings_with_runtime_dir(&runtime_dir_str);
    settings
        .save_to_file(&paths.config_dir().join("settings.toml"))
        .unwrap();

    let supervisor = BusytokSupervisor::with_adapters_and_settings(db, paths, vec![], settings);
    let envelope = supervisor.settings_diagnostics().await.unwrap();
    let sub = envelope.data.subagent.unwrap();

    // The 3 checks that previously failed in dev must now pass.
    for check_name in ["bundled_node_arch", "bundle_manifest_readable", "pi_runtime_installed"] {
        let check = sub
            .checks
            .iter()
            .find(|c| c.name == check_name)
            .unwrap_or_else(|| panic!("missing check: {check_name}"));
        assert_eq!(
            check.status, "ok",
            "check {} should be ok with complete resources: {:?}",
            check_name, check.detail
        );
    }

    supervisor.shutdown_writer().await.unwrap();
}

#[tokio::test]
#[serial]
async fn doctor_bundled_node_arch_fails_when_arch_dir_missing() {
    let tmp = tempfile::tempdir().unwrap();
    let runtime_dir = tmp.path().join("pi-sidecar");
    fs::create_dir_all(&runtime_dir).unwrap();
    fs::write(
        runtime_dir.join("pi-sidecar.bundle.js"),
        "// stub",
    )
    .unwrap();
    fs::write(
        runtime_dir.join("manifest.json"),
        SidecarManifest {
            version: "1".to_string(),
            protocol_version: 1,
            bundle: "pi-sidecar.bundle.js".to_string(),
            node_runtime_version: "22.6.0".to_string(),
        }
        .to_json_string(),
    )
    .unwrap();
    // NO node/ directory — the bundled_node_arch check must fail.

    let db = Database::open_in_memory().unwrap();
    let paths = BusytokPaths::for_test(tmp.path());
    let settings = make_settings_with_runtime_dir(
        &runtime_dir.to_string_lossy(),
    );
    settings
        .save_to_file(&paths.config_dir().join("settings.toml"))
        .unwrap();

    let supervisor = BusytokSupervisor::with_adapters_and_settings(db, paths, vec![], settings);
    let envelope = supervisor.settings_diagnostics().await.unwrap();
    let sub = envelope.data.subagent.unwrap();
    let check = sub
        .checks
        .iter()
        .find(|c| c.name == "bundled_node_arch")
        .unwrap();
    assert_eq!(check.status, "error");
    assert!(check.detail.as_deref().unwrap_or("").contains("not found"));

    supervisor.shutdown_writer().await.unwrap();
}

#[tokio::test]
#[serial]
async fn doctor_manifest_rejects_missing_node_runtime_version_field() {
    let tmp = tempfile::tempdir().unwrap();
    let runtime_dir = tmp.path().join("pi-sidecar");
    fs::create_dir_all(&runtime_dir).unwrap();
    fs::write(runtime_dir.join("pi-sidecar.bundle.js"), "// stub").unwrap();
    // Manifest missing node_runtime_version — must be rejected by the
    // typed SidecarManifest deserialize (Task 1).
    fs::write(
        runtime_dir.join("manifest.json"),
        r#"{"version":"1","protocol_version":1,"bundle":"pi-sidecar.bundle.js"}"#,
    )
    .unwrap();

    let db = Database::open_in_memory().unwrap();
    let paths = BusytokPaths::for_test(tmp.path());
    let settings = make_settings_with_runtime_dir(
        &runtime_dir.to_string_lossy(),
    );
    settings
        .save_to_file(&paths.config_dir().join("settings.toml"))
        .unwrap();

    let supervisor = BusytokSupervisor::with_adapters_and_settings(db, paths, vec![], settings);
    let envelope = supervisor.settings_diagnostics().await.unwrap();
    let sub = envelope.data.subagent.unwrap();
    let check = sub
        .checks
        .iter()
        .find(|c| c.name == "bundle_manifest_readable")
        .unwrap();
    assert_eq!(check.status, "error");
    assert!(check.detail.as_deref().unwrap_or("").contains("SidecarManifest"));

    supervisor.shutdown_writer().await.unwrap();
}
```

- [ ] **Step 2: Run the tests**

Run: `cargo test -p busytok-runtime --test sidecar_bundling_doctor`
Expected: 3 tests pass.

- [ ] **Step 3: Run the full workspace suite to verify no regressions**

Run: `cargo test --workspace`
Expected: all pass.

- [ ] **Step 4: Manual smoke checklist (not automated — for the human operator)**

```markdown
## Manual Verification (post-merge, on a packaged build)

1. `./packaging/macos/scripts/package_dmg.sh` produces `Busytok.app` with:
   - `Contents/Resources/pi-sidecar/pi-sidecar.bundle.js` present
   - `Contents/Resources/pi-sidecar/manifest.json` valid JSON with `protocol_version=1`
   - `Contents/Resources/pi-sidecar/node/aarch64/node` + `node/x86_64/node` present + executable
2. `codesign -d --entitlements - Busytok.app/Contents/Resources/pi-sidecar/node/aarch64/node` shows `allow-jit` only (no `allow-unsigned-executable-memory`).
3. Fresh install: `busytok doctor` → all checks `ok` (no `error`, no `warning`).
4. Fresh install: `busytok delegate --profile pi/search-cheap --prompt "echo hello"` returns model output.
5. DMG size < 150MB (`du -sh Busytok-*.dmg`).
```

- [ ] **Step 5: Commit**

```bash
git add crates/busytok-runtime/tests/sidecar_bundling_doctor.rs
git commit -m "test(runtime): add sidecar bundling doctor integration tests"
```

---

## Self-Review Checklist

[Run this after writing the complete plan — not a subagent dispatch.]

**Spec coverage:**
- ✅ `.app` structure (§351-358): Task 2 creates `Contents/Resources/pi-sidecar/{bundle.js, manifest.json, node/{aarch64,x86_64}/node}`.
- ✅ `manifest.json` schema (§360-368): Task 1 defines `SidecarManifest` struct with all 4 fields; Task 2 generates conforming JSON.
- ✅ Runtime path resolution (§370-373): Task 3 persists `runtime_dir` via GUI; no `current_exe()` in the service (the service reads from `settings.toml`).
- ✅ Signing order (§375-381): Task 2 inserts node-binary signing between helper signing and `.app` re-seal.
- ✅ Entitlements `allow-jit` only (§383): Task 2 creates `sidecar-node.plist` with `allow-jit` only; Task 4's smoke test asserts `allow-unsigned-executable-memory` is absent.
- ✅ Dual-architecture (§385-388): Task 2 downloads + bundles both arches.
- ✅ DMG size < 150MB (§390): noted in manual smoke checklist.
- ✅ Doctor checks pass (§392-409): Task 5 verifies the 3 previously-failing checks now pass; Task 3 enables `pi_sidecar.enabled=true` so `protocol_version` moves to `ok`.
- ✅ `release_script_smoke.sh` verify checks (§411-414): Task 4 adds 6 contract checks (bundle source call, manifest generator, dual-arch download, entitlements, signing).
- ✅ No new doctor check added (§394): confirmed — Task 5 only adds tests for existing checks.
- ✅ Deferred items (§416-420): Node SEA, per-arch DMG, auto-update, CI e2e — all explicitly out of scope.

**Placeholder scan:** No "TBD", "TODO", "implement later", "add appropriate error handling", "similar to Task N" — all steps contain complete code.

**Type consistency:** `SidecarManifest` (Task 1) → generated by `_bundle_sidecar.sh` (Task 2) → deserialized by doctor check (Task 1 Step 4) → asserted in integration test (Task 5). Field names match: `version`, `protocol_version`, `bundle`, `node_runtime_version`. `protocol_version` is `u32` everywhere (matches `PROTOCOL_VERSION: u32` at protocol.rs:28 — direct assignment in Task 5's test compiles without a cast).

**Architectural reuse:**
- `BusytokPaths::sidecar_*` methods (paths.rs:160-192) — reused as-is, no new locator.
- `SubagentPiSidecarConfig.runtime_dir` (lib.rs:257) — reused as-is.
- `_bundle_helpers.sh` pattern (`_absolute_app_bundle_path` + `bundle_*_into_app`) — extended with `_bundle_sidecar.sh`.
- `BusytokSettings::load()` + `save_to_file()` — reused for GUI persistence.
- `tracing::warn!`/`info!` with `event_code` — matches the existing logging convention.
- `DoctorCheckDto` + `settings.diagnostics` RPC — unchanged.

**Test coverage estimate:** Task 1 adds 5 unit tests (manifest round-trip + reject-missing + reject-wrong-type + pretty-print). Task 3 adds 5 unit tests (3 path-resolution + 2 persistence — calls the REAL production free functions, NOT a mirror). Task 5 adds 3 integration tests (all-resources-present + missing-node + malformed-manifest). Task 4 adds 6 shell contract checks. Total: ~19 new test cases covering the new code paths. The existing doctor tests (subagent_e2e_sidecar.rs) continue to cover the negative paths.

**Sub-agent review fixes applied (round 1):**
- 🔴 C1 (Critical): `SidecarManifest.protocol_version` changed `i64` → `u32` to match `PROTOCOL_VERSION: u32` at `protocol.rs:28`. Updated in: Task 1 Interfaces section, struct field doc + type, Task 2 shell grep comment. Task 5's test now assigns `PROTOCOL_VERSION` directly without a cast (compiles).
- 🔴 C2 (Critical): Added Task 1 Step 5 to update the existing `doctor_bundle_manifest_readable_check_validates_manifest` test, which wrote a non-conforming `{"name":"pi-sidecar","version":"0.1.0"}` manifest. Now writes schema-conforming JSON. Task 1 steps renumbered (5→6, 6→7, 7→8); commit step adds `subagent_e2e_sidecar.rs` to `git add`.
- 🟡 I1: Documented that Task 4 uses static analysis (grep) matching the existing `release_script_smoke.sh` convention; runtime output verification is in Task 5's manual smoke checklist.
- 🟡 I2: Added explicit `|| { echo ...; return 1; }` error checking after `tar`/`mv`/`cp`/`chmod` in `_bundle_sidecar.sh`, because `set -e` is disabled inside functions invoked via `|| return 1`. Removed dead `strip_prefix` local var.
- 🟡 I3: Documented the `settings.toml` lost-update race as accepted (idempotent mutations, self-healing on next launch).
- 🟢 M1+M2: Fixed stale docstring line number (251→252) + added the service-only path fix (`lib/busytok/sidecars/pi` → `lib/busytok/pi-sidecar`).
- 🟢 M3: Removed dead `SIDECAR_NODE_MAJOR` variable + its helper-comment reference.
- 🟢 M4: Reused `paths_for_lc` in Task 3 Step 4 instead of a redundant `BusytokPaths::new()`.
- 🟢 M5: Resolved Task 5 Files section contradiction (removed `subagent_e2e_sidecar.rs` from "Modify" list; clarified it's updated in Task 1 Step 5, not Task 5).
- 🟢 M6: `make_settings_with_runtime_dir` intentionally deviates from `make_sidecar_settings` (sets `enabled=false`, no providers) because it tests filesystem doctor checks, not delegate — kept as-is with a clear docstring.

**Sub-agent review fixes applied (round 2):**
- 🔴 MUST: Extracted `resolve_sidecar_dir_from_exe(exe: &Path)` as a production free function. Tests now call the REAL function (not a mirror) — production path resolver has direct automated test coverage (was 0%). Task 3 rewritten with proper TDD (red → green).
- 🟡 Should: Added `chmod +x packaging/macos/scripts/_bundle_sidecar.sh` in Task 2 Step 5 (smoke test checks `[ -x "$HELPER" ]`).
- 🟡 Should: Added `grep -rn 'sidecars/pi' crates/ apps/ packaging/` verification in Task 1 Step 2 to catch other stale references.
- 🟡 Should: Documented deliberate deviation from spec §371 "via Tauri resource API" in Task 3 decision paragraph + `resolve_packaged_sidecar_dir` docstring.
- 🟢 Optional: Switched from inline `#[cfg(test)] mod phase5_tests { }` to file-module form (`phase5_tests.rs` + `#[cfg(test)] mod phase5_tests;` in lib.rs) — matches the existing 14 test-module convention in lib.rs.
- 🟢 Optional: Added "Coverage scope (explicit)" note in Task 5 — protocol_version pass state requires real Node + sidecar, verified by manual smoke checklist; >90% target applies to NEW code, not entire doctor path.

**Sub-agent review fixes applied (round 3):**
- 🔴 P0: Fixed Node download helper — split upstream archive token (`darwin-arm64` / `darwin-x64`) from app-internal dir name (`aarch64` / `x86_64`). Previously conflated into one variable (`aarch64-apple-darwin`), which would fail to download/extract (Node.js release archives use `darwin-arm64`, NOT `aarch64-apple-darwin`). Added `NODE_ARCHES` array with explicit `<upstream_token> <app_dir>` pairs. Loop iterates the array, passing both to `download_node_binary`.
- 🔴 P1: Replaced direct file write with service-owned two-phase update. The daemon holds settings in `Arc<Mutex<BusytokSettings>>` (supervisor.rs:334) — a direct file write leaves the running daemon's in-memory state stale. Added `pi_sidecar_locator_update` method on `BusytokSupervisor` (mirrors `provider_update`: clone → mutate → save → swap in-memory) + `PiSidecarLocatorUpdateRequestDto`/`ResponseDto` + `RuntimeControl` trait method + dispatch case + `TestRuntimeControl` stub. GUI calls it via `invoke_busytok` RPC; falls back to direct file write only when service is unreachable (cold-start). Integration test `pi_sidecar_locator_update_mutates_in_memory_and_disk` verifies BOTH the in-memory swap AND the file persist (the P1 regression guard). Added `#[doc(hidden)] pub fn pi_sidecar_state` test accessor (the `settings` field is private; `settings_snapshot` RPC returns a DTO that omits `pi_sidecar.runtime_dir`).
- 🔴 P1: Made sidecar bundle build unconditional in `bundle_sidecar_resources_into_app` (was conditional on `dist/` missing — would silently package stale dist output, breaking release determinism). The pi-sidecar package is small (~100ms build); unconditional rebuild is cheap.
- Note: Smoke test currently does static greps for the download URL tokens (consistent with existing `release_script_smoke.sh` convention). The P0 fix's real download verification is covered by the manual smoke checklist in Task 5 Step 4 (running `package_dmg.sh` for real and inspecting `Contents/Resources/pi-sidecar/node/`).

**Sub-agent review fixes applied (round 4):**
- 🔴 P1: Rewrote the Architecture header to match the final Task 3 design. Removed stale references to `resource_dir()` + `settings_update` RPC (the old approach that was replaced by `pi_sidecar_locator_update`). The header now accurately describes: `current_exe()` walk-up → `pi_sidecar_locator_update` service-owned RPC primary → file fallback ONLY for cold-start transport-unreachability. Implementers reading the header first will no longer be led to the wrong path.
- 🔴 P1: Tightened the file fallback condition in `refresh_sidecar_locator`. Previously `if rpc_result.is_ok() { ... } else { fallback }` would bypass the service on ANY RPC error (including business errors), masking real bugs and re-introducing state drift. Now uses `is_transport_unreachable(err)` to classify: transport/bootstrap errors (`"connect/bootstrap phase timed out"`, `"service unavailable:"`, `"service bootstrap failed:"`, `"call to '...' timed out"`) → file fallback; service business errors (`"[{code}] {message}"`, `"dispatch error:"`) → logged at `error!` level + surfaced (NO fallback). Added `classifies_transport_vs_business_errors` unit test covering 4 transport cases + 4 business-error cases + edge (empty string).
