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
#[serde(deny_unknown_fields)]
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
        serde_json::to_string_pretty(self).expect("SidecarManifest serialization is infallible")
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

    #[test]
    fn manifest_rejects_unknown_field() {
        // A typo'd field (e.g. "extra") must be rejected so generation bugs
        // are surfaced instead of silently ignored. With `deny_unknown_fields`,
        // any field not in the struct is a deserialize error.
        let json = r#"{"version":"1","protocol_version":1,"bundle":"pi-sidecar.bundle.js","node_runtime_version":"22.6.0","extra":"unknown"}"#;
        let result: Result<SidecarManifest, _> = serde_json::from_str(json);
        assert!(result.is_err(), "unknown field must be rejected");
    }
}
