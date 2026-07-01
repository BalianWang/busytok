#![allow(clippy::unwrap_used)]

use busytok_subagent::sidecar::{SidecarRequest, SidecarResponse, SidecarRpcError};
use serde_json::json;

#[test]
fn request_serializes_to_jsonrpc20() {
    let req = SidecarRequest::new("adapter.initialize", json!({"protocol_version": 1}), 1);
    let s = serde_json::to_string(&req).unwrap();
    assert_eq!(
        s,
        r#"{"jsonrpc":"2.0","method":"adapter.initialize","params":{"protocol_version":1},"id":1}"#
    );
}

#[test]
fn response_with_result_deserializes() {
    let raw = r#"{"jsonrpc":"2.0","result":{"status":"healthy"},"id":1}"#;
    let resp: SidecarResponse = serde_json::from_str(raw).unwrap();
    assert_eq!(resp.id, 1);
    assert!(resp.result.is_some());
    assert!(resp.error.is_none());
}

#[test]
fn response_with_error_deserializes() {
    let raw = r#"{"jsonrpc":"2.0","error":{"code":-32004,"message":"unhealthy"},"id":2}"#;
    let resp: SidecarResponse = serde_json::from_str(raw).unwrap();
    let err = resp.error.unwrap();
    assert_eq!(err.code, -32004);
    assert_eq!(err.message, "unhealthy");
}

#[test]
fn error_code_constants_match_spec() {
    use busytok_subagent::sidecar::*;
    assert_eq!(SESSION_NOT_FOUND, -32001);
    assert_eq!(PROTOCOL_MISMATCH, -32008);
}

#[test]
fn sidecar_application_error_maps_to_subagent_error() {
    use busytok_subagent::sidecar::{
        SidecarError, PROFILE_NOT_FOUND, SESSION_NOT_FOUND, TASK_TIMEOUT,
    };
    use busytok_subagent::SubagentError;

    let e: SubagentError =
        SidecarError::Application(SESSION_NOT_FOUND, "no such session".into(), None).into();
    assert!(matches!(e, SubagentError::NotFound(_)));
    assert_eq!(e.code(), "subagent.not_found");

    let e: SubagentError =
        SidecarError::Application(PROFILE_NOT_FOUND, "bad profile".into(), None).into();
    assert!(matches!(e, SubagentError::ProfileNotFound(_)));

    let e: SubagentError = SidecarError::Application(TASK_TIMEOUT, "slow".into(), None).into();
    assert!(matches!(e, SubagentError::TaskTimeout));
    assert_eq!(e.code(), "subagent.task_timeout");

    let e: SubagentError = SidecarError::Spawn("no node".into()).into();
    assert!(matches!(e, SubagentError::SidecarSpawn(_)));
    assert_eq!(e.code(), "subagent.sidecar_spawn_failed");

    // IO errors (stdin/stdout pipe closed) must map to SidecarIo, NOT
    // SidecarRpc — the two were previously indistinguishable.
    let e: SubagentError = SidecarError::Io("pipe closed".into()).into();
    assert!(matches!(e, SubagentError::SidecarIo(_)));
    assert_eq!(e.code(), "subagent.sidecar_io_error");
}

/// Cover the remaining `From<SidecarError>` arms: Rpc, Timeout, Crashed, and
/// the `_ =>` catch-all for Application codes that don't have a dedicated
/// domain variant (SIDECAR_UNHEALTHY, TOOL_NOT_ALLOWED, INVALID_OUTPUT_SCHEMA,
/// PROTOCOL_MISMATCH). These surface as `SidecarRpc("[code] msg")` so the
/// control layer still reports them. HOT_SESSION_LIMIT_REACHED now has a
/// dedicated variant — covered by `hot_session_limit_extracts_candidate_from_data`.
#[test]
fn sidecar_error_remaining_variants_map_to_subagent_error() {
    use busytok_subagent::sidecar::{SidecarError, PROTOCOL_MISMATCH, SIDECAR_UNHEALTHY};
    use busytok_subagent::SubagentError;

    // Rpc → SidecarRpc (preserves message verbatim, code = None).
    let e: SubagentError = SidecarError::Rpc("serialize boom".into()).into();
    match &e {
        SubagentError::SidecarRpc { message, code } => {
            assert_eq!(message, "serialize boom");
            assert!(code.is_none(), "raw Rpc has no structured code");
        }
        other => panic!("expected SidecarRpc, got {other:?}"),
    }
    assert_eq!(e.code(), "subagent.sidecar_rpc_error");

    // Timeout → SidecarTimeout.
    let e: SubagentError = SidecarError::Timeout("adapter.health".into()).into();
    match &e {
        SubagentError::SidecarTimeout(msg) => assert_eq!(msg, "adapter.health"),
        other => panic!("expected SidecarTimeout, got {other:?}"),
    }
    assert_eq!(e.code(), "subagent.sidecar_timeout");

    // Crashed → SidecarCrashed.
    let e: SubagentError = SidecarError::Crashed("stdout closed".into()).into();
    match &e {
        SubagentError::SidecarCrashed(msg) => assert_eq!(msg, "stdout closed"),
        other => panic!("expected SidecarCrashed, got {other:?}"),
    }
    assert_eq!(e.code(), "subagent.sidecar_crashed");

    // Unmatched Application codes → SidecarRpc with "[code] msg" formatting.
    // Each of these exercises the `_ =>` catch-all arm. The numeric code is
    // preserved in the structured `code` field.
    for (code, label) in [
        (SIDECAR_UNHEALTHY, "unhealthy"),
        (PROTOCOL_MISMATCH, "mismatch"),
    ] {
        let e: SubagentError = SidecarError::Application(code, label.into(), None).into();
        match &e {
            SubagentError::SidecarRpc { message, code: c } => {
                assert_eq!(*c, Some(code), "structured code must be preserved");
                assert!(
                    message.contains(&format!("[{code}]")),
                    "expected '[{code}]' prefix in '{message}'"
                );
                assert!(message.contains(label), "expected '{label}' in '{message}'");
            }
            other => panic!("expected SidecarRpc for code {code}, got {other:?}"),
        }
        assert_eq!(e.code(), "subagent.sidecar_rpc_error");
    }
}

/// HOT_SESSION_LIMIT_REACHED now maps to a dedicated `SubagentError::HotSessionLimit`
/// variant. The sidecar is the hot-pool authority (spec §4.4) — its
/// `data.candidate` names the LRU session to evict, so the `From<SidecarError>`
/// conversion must extract that field rather than discarding it. Covers both
/// the with-data and the missing-data (default-empty) cases.
#[test]
fn hot_session_limit_extracts_candidate_from_data() {
    use busytok_subagent::sidecar::{SidecarError, HOT_SESSION_LIMIT_REACHED};
    use busytok_subagent::SubagentError;

    // With data.candidate present → candidate propagates verbatim.
    let e: SubagentError = SidecarError::Application(
        HOT_SESSION_LIMIT_REACHED,
        "hot limit".into(),
        Some(json!({"candidate": "sess_abc"})),
    )
    .into();
    match &e {
        SubagentError::HotSessionLimit { candidate } => {
            assert_eq!(candidate, "sess_abc");
        }
        other => panic!("expected HotSessionLimit, got {other:?}"),
    }
    assert_eq!(e.code(), "subagent.hot_session_limit");

    // Missing data → default-empty candidate (no panic, no SidecarRpc fallback).
    let e: SubagentError =
        SidecarError::Application(HOT_SESSION_LIMIT_REACHED, "hot limit".into(), None).into();
    match &e {
        SubagentError::HotSessionLimit { candidate } => {
            assert_eq!(candidate, "");
        }
        other => panic!("expected HotSessionLimit with empty candidate, got {other:?}"),
    }
    assert_eq!(e.code(), "subagent.hot_session_limit");
}

// `SidecarRpcError` is part of the protocol surface; verify it round-trips
// through serde so future changes to its fields don't silently break the
// JSON-RPC error envelope contract.
#[test]
fn sidecar_rpc_error_round_trips() {
    let err = SidecarRpcError {
        code: -32001,
        message: "missing".into(),
        data: Some(json!({"session": "abc"})),
    };
    let s = serde_json::to_string(&err).unwrap();
    let back: SidecarRpcError = serde_json::from_str(&s).unwrap();
    assert_eq!(back.code, -32001);
    assert_eq!(back.message, "missing");
    assert!(back.data.is_some());
}
