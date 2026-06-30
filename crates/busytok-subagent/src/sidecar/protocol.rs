//! JSON-RPC 2.0 protocol types for the Pi sidecar channel.
//!
//! Framing: newline-delimited JSON (one JSON object per line) over stdio.
//! This is a separate channel from busytok-control (which uses length-prefixed
//! framing) — the sidecar protocol is canonical JSON-RPC 2.0 over stdio.

use serde::{Deserialize, Serialize};

// Application error codes (spec §4.2)
pub const SESSION_NOT_FOUND: i32 = -32001;
pub const HOT_SESSION_LIMIT_REACHED: i32 = -32002;
pub const TASK_TIMEOUT: i32 = -32003;
pub const SIDECAR_UNHEALTHY: i32 = -32004;
pub const PROFILE_NOT_FOUND: i32 = -32005;
pub const TOOL_NOT_ALLOWED: i32 = -32006;
pub const INVALID_OUTPUT_SCHEMA: i32 = -32007;
pub const PROTOCOL_MISMATCH: i32 = -32008;
// Phase 3 Task 3: error classification codes. Start at -32010 to avoid
// collisions with the existing -32001..-32008 range. Mapped to
// `TaskErrorKind` by `classify_sidecar_error` in executor.rs.
/// Auth failure (401) → `TaskErrorKind::Auth` → hard kill + remove worker.
pub const AUTH_FAILURE: i32 = -32010;
/// Rate limit (429) → `TaskErrorKind::RateLimit` → keep worker, surface error.
pub const RATE_LIMIT: i32 = -32011;
/// Network error → `TaskErrorKind::Network` → keep worker, surface error.
pub const NETWORK_ERROR: i32 = -32012;

pub const PROTOCOL_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SidecarRequest {
    pub jsonrpc: String,
    pub method: String,
    pub params: serde_json::Value,
    pub id: u64,
}

impl SidecarRequest {
    pub fn new(method: &str, params: serde_json::Value, id: u64) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            method: method.to_string(),
            params,
            id,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SidecarResponse {
    pub jsonrpc: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<SidecarRpcError>,
    pub id: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SidecarRpcError {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}
