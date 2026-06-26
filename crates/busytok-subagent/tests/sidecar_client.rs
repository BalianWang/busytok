//! Error-path tests for `SidecarRpcClient`.
//!
//! The client requires `tokio::process::{ChildStdin, ChildStdout}`, so we
//! spawn small bash scripts that exercise the timeout / stdout-closed /
//! id-mismatch / notification-skip / parse-skip / application-error branches
//! of `call_with_timeout`. These branches are uncovered by the supervisor
//! integration tests (which only exercise the happy path through the mock
//! sidecar fixture).

#![allow(clippy::unwrap_used)]

use std::time::Duration;

use busytok_subagent::sidecar::{SidecarError, SidecarRpcClient};
use tokio::process::Command;

/// Spawn `script` under bash with piped stdio and return `(child, client)`.
/// `kill_on_drop(true)` ensures the child is reaped when the test ends.
async fn spawn_client(script: &str) -> (tokio::process::Child, SidecarRpcClient) {
    let mut cmd = Command::new("bash");
    cmd.arg("-c").arg(script);
    cmd.stdin(std::process::Stdio::piped());
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::null());
    cmd.kill_on_drop(true);
    let mut child = cmd.spawn().unwrap();
    let stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let client = SidecarRpcClient::new(stdin, stdout);
    (child, client)
}

/// Bash snippet to extract the numeric `id` from a single-line JSON-RPC
/// request read from stdin. Mirrors the pattern used by the mock-sidecar
/// fixture so we don't depend on `jq`.
const EXTRACT_ID: &str =
    r#"ID=$(printf '%s' "$LINE" | sed -n 's/.*"id"[[:space:]]*:[[:space:]]*\([0-9]*\).*/\1/p')"#;

#[tokio::test]
async fn call_with_timeout_returns_timeout_when_response_is_slow() {
    // Script reads the request then sleeps — the 1ms timeout fires while
    // waiting for the response line.
    let script = "IFS= read -r LINE; sleep 1";
    let (_child, mut client) = spawn_client(script).await;
    let err = client
        .call_with_timeout(
            "adapter.health",
            serde_json::json!({}),
            Duration::from_millis(1),
        )
        .await
        .unwrap_err();
    match err {
        SidecarError::Timeout(method) => {
            assert_eq!(method, "adapter.health");
        }
        other => panic!("expected SidecarError::Timeout, got {other:?}"),
    }
}

#[tokio::test]
async fn call_returns_crashed_when_stdout_closes() {
    // Script reads the request then exits 0, closing stdout. The client's
    // read_line returns Ok(0) → SidecarError::Crashed("sidecar stdout closed").
    let script = "IFS= read -r LINE; exit 0";
    let (_child, mut client) = spawn_client(script).await;
    let err = client
        .call_with_timeout(
            "adapter.health",
            serde_json::json!({}),
            Duration::from_secs(5),
        )
        .await
        .unwrap_err();
    match err {
        SidecarError::Crashed(msg) => {
            assert!(
                msg.contains("stdout closed"),
                "expected stdout-closed message, got: {msg}"
            );
        }
        other => panic!("expected SidecarError::Crashed, got {other:?}"),
    }
}

#[tokio::test]
async fn call_skips_response_with_mismatched_id_and_returns_matching() {
    // Script sends a response with a bogus id (99999) then the correct
    // response. The client must skip the mismatched one and return the
    // matching result — not return the wrong-id result, and not time out.
    let script = format!(
        r#"IFS= read -r LINE
{EXTRACT_ID}
printf '{{"jsonrpc":"2.0","result":{{"wrong":true}},"id":99999}}\n'
printf '{{"jsonrpc":"2.0","result":{{"ok":true}},"id":%s}}\n' "$ID"
sleep 1"#
    );
    let (_child, mut client) = spawn_client(&script).await;
    let result = client
        .call_with_timeout(
            "adapter.health",
            serde_json::json!({}),
            Duration::from_secs(5),
        )
        .await
        .expect("client should skip mismatched id and return matching response");
    assert_eq!(result["ok"], serde_json::json!(true));
}

#[tokio::test]
async fn call_skips_notification_line_and_returns_matching_response() {
    // Script sends a notification (method present, no id) then the correct
    // response. The client must skip the notification (Plan 2 does not
    // consume task.event) and return the matching result.
    let script = format!(
        r#"IFS= read -r LINE
{EXTRACT_ID}
printf '{{"jsonrpc":"2.0","method":"task.event","params":{{"foo":1}}}}\n'
printf '{{"jsonrpc":"2.0","result":{{"ok":true}},"id":%s}}\n' "$ID"
sleep 1"#
    );
    let (_child, mut client) = spawn_client(&script).await;
    let result = client
        .call_with_timeout(
            "adapter.health",
            serde_json::json!({}),
            Duration::from_secs(5),
        )
        .await
        .expect("client should skip notification and return matching response");
    assert_eq!(result["ok"], serde_json::json!(true));
}

#[tokio::test]
async fn call_skips_unparseable_line_and_returns_matching_response() {
    // Script sends unparseable garbage then the correct response. The client
    // logs and skips the garbage line, then returns the matching result.
    let script = format!(
        r#"IFS= read -r LINE
{EXTRACT_ID}
printf 'this is not json\n'
printf '{{"jsonrpc":"2.0","result":{{"ok":true}},"id":%s}}\n' "$ID"
sleep 1"#
    );
    let (_child, mut client) = spawn_client(&script).await;
    let result = client
        .call_with_timeout(
            "adapter.health",
            serde_json::json!({}),
            Duration::from_secs(5),
        )
        .await
        .expect("client should skip unparseable line and return matching response");
    assert_eq!(result["ok"], serde_json::json!(true));
}

#[tokio::test]
async fn call_skips_empty_line_and_returns_matching_response() {
    // Script sends an empty line (just newline) then the correct response.
    // The client must skip the empty line (trimmed.is_empty()) and return
    // the matching result.
    let script = format!(
        r#"IFS= read -r LINE
{EXTRACT_ID}
printf '\n'
printf '{{"jsonrpc":"2.0","result":{{"ok":true}},"id":%s}}\n' "$ID"
sleep 1"#
    );
    let (_child, mut client) = spawn_client(&script).await;
    let result = client
        .call_with_timeout(
            "adapter.health",
            serde_json::json!({}),
            Duration::from_secs(5),
        )
        .await
        .expect("client should skip empty line and return matching response");
    assert_eq!(result["ok"], serde_json::json!(true));
}

#[tokio::test]
async fn call_returns_application_error_when_sidecar_responds_with_error() {
    // Script sends a JSON-RPC error response with code -32005 (PROFILE_NOT_FOUND).
    // The client must surface it as SidecarError::Application(-32005, msg).
    let script = format!(
        r#"IFS= read -r LINE
{EXTRACT_ID}
printf '{{"jsonrpc":"2.0","error":{{"code":-32005,"message":"profile not found"}},"id":%s}}\n' "$ID"
sleep 1"#
    );
    let (_child, mut client) = spawn_client(&script).await;
    let err = client
        .call_with_timeout(
            "session.turn_auto",
            serde_json::json!({}),
            Duration::from_secs(5),
        )
        .await
        .unwrap_err();
    match err {
        SidecarError::Application(code, msg, _) => {
            assert_eq!(code, -32005);
            assert!(msg.contains("profile not found"), "got: {msg}");
        }
        other => panic!("expected SidecarError::Application, got {other:?}"),
    }
}
