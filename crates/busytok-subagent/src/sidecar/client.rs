//! JSON-RPC 2.0 client over stdio (newline-delimited).

use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{ChildStdin, ChildStdout};
use tokio::time::timeout;
use tracing::debug;

use crate::sidecar::protocol::{SidecarRequest, SidecarResponse};
use crate::sidecar::SidecarError;

const DEFAULT_CALL_TIMEOUT: Duration = Duration::from_secs(30);

pub struct SidecarRpcClient {
    stdin: ChildStdin,
    reader: BufReader<ChildStdout>,
    next_id: u64,
}

impl SidecarRpcClient {
    pub fn new(stdin: ChildStdin, stdout: ChildStdout) -> Self {
        Self {
            stdin,
            reader: BufReader::new(stdout),
            next_id: 1,
        }
    }

    /// Send a JSON-RPC request and wait for the matching response.
    pub async fn call(
        &mut self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, SidecarError> {
        self.call_with_timeout(method, params, DEFAULT_CALL_TIMEOUT)
            .await
    }

    pub async fn call_with_timeout(
        &mut self,
        method: &str,
        params: serde_json::Value,
        dur: Duration,
    ) -> Result<serde_json::Value, SidecarError> {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        let req = SidecarRequest::new(method, params, id);
        let mut line = serde_json::to_string(&req)
            .map_err(|e| SidecarError::Rpc(format!("serialize: {e}")))?;
        line.push('\n');
        self.stdin
            .write_all(line.as_bytes())
            .await
            .map_err(|e| SidecarError::Io(e.to_string()))?;
        self.stdin
            .flush()
            .await
            .map_err(|e| SidecarError::Io(e.to_string()))?;

        // Read lines until we see the response with our id. Notifications
        // (method present, no id) are skipped at debug level — Plan 2 does not
        // consume task.event; Plan 3+ will route them to a notification sink.
        let deadline = tokio::time::Instant::now() + dur;
        loop {
            let mut buf = String::new();
            let remaining = deadline
                .checked_duration_since(tokio::time::Instant::now())
                .ok_or_else(|| SidecarError::Timeout(method.to_string()))?;
            match timeout(remaining, self.reader.read_line(&mut buf)).await {
                Err(_) => return Err(SidecarError::Timeout(method.to_string())),
                Ok(Err(e)) => return Err(SidecarError::Io(e.to_string())),
                Ok(Ok(0)) => {
                    return Err(SidecarError::Crashed("sidecar stdout closed".to_string()))
                }
                Ok(Ok(_)) => {}
            }
            let trimmed = buf.trim();
            if trimmed.is_empty() {
                continue;
            }
            // Parse as a generic Value first to discriminate notification vs response.
            let val: serde_json::Value = match serde_json::from_str(trimmed) {
                Ok(v) => v,
                Err(e) => {
                    debug!(
                        event_code = "subagent.sidecar.client.parse_skipped",
                        error = %e,
                        "skipping unparseable line"
                    );
                    continue;
                }
            };
            // Notification: has "method" but no "id". Skip (do not consume).
            if val.get("method").is_some() && val.get("id").is_none() {
                debug!(
                    event_code = "subagent.sidecar.client.notification_skipped",
                    method = %val["method"],
                    "sidecar notification skipped (not consumed in MVP)"
                );
                continue;
            }
            // Response: must have "id". Check match.
            let resp_id = val.get("id").and_then(|v| v.as_u64());
            if resp_id != Some(id) {
                debug!(
                    event_code = "subagent.sidecar.client.id_mismatch",
                    expected = id,
                    got = ?resp_id,
                    "out-of-order response, skipping"
                );
                continue;
            }
            let resp: SidecarResponse = serde_json::from_value(val)
                .map_err(|e| SidecarError::Rpc(format!("deserialize: {e}")))?;
            if let Some(err) = resp.error {
                return Err(SidecarError::Application(err.code, err.message, err.data));
            }
            return Ok(resp.result.unwrap_or(serde_json::Value::Null));
        }
    }
}
