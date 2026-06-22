//! Local IPC client for connecting to the Busytok control server.
//!
//! The client connects to a platform transport endpoint, sends a hello
//! handshake, then makes RPC calls using length-prefixed JSON frames.

use anyhow::{Context, Result};
use busytok_protocol::dto::*;

use crate::protocol::{read_frame, write_frame, HELLO, HELLO_ACK};
use crate::transport::{ControlTransport, PlatformTransport};

/// Local IPC control client.
pub struct ControlClient<T: ControlTransport = PlatformTransport> {
    reader: tokio::io::ReadHalf<T::Stream>,
    writer: tokio::io::WriteHalf<T::Stream>,
    buf: Vec<u8>,
}

impl<T: ControlTransport> ControlClient<T> {
    /// Connect to the control server at the given transport endpoint.
    pub async fn connect(endpoint: impl AsRef<str>) -> Result<Self> {
        let endpoint = endpoint.as_ref();
        let stream = T::connect(endpoint)
            .await
            .with_context(|| format!("connecting to {endpoint}"))?;
        let (reader, writer) = tokio::io::split(stream);

        let mut client = Self {
            reader,
            writer,
            buf: Vec::new(),
        };

        // Send hello handshake.
        write_frame(&mut client.writer, HELLO)
            .await
            .context("sending hello handshake")?;

        // Wait for hello acknowledgment.
        let ack = read_frame(&mut client.reader, &mut client.buf)
            .await
            .context("reading hello acknowledgment")?;
        if ack != HELLO_ACK {
            anyhow::bail!("invalid handshake ack: expected '{HELLO_ACK}', got '{ack}'");
        }

        Ok(client)
    }

    /// Send a control request and wait for the response.
    pub async fn call(&mut self, request: ControlRequest) -> Result<ControlResponse> {
        let request_json = serde_json::to_string(&request)?;
        write_frame(&mut self.writer, &request_json)
            .await
            .context("sending request")?;

        let response_json = read_frame(&mut self.reader, &mut self.buf)
            .await
            .context("reading response")?;
        let response: ControlResponse =
            serde_json::from_str(&response_json).context("parsing response")?;
        Ok(response)
    }

    /// Subscribe to events. Returns the subscription acknowledgment.
    /// After this, use `recv_event_batch` to receive streamed events.
    pub async fn subscribe(&mut self, types: Vec<String>) -> Result<ControlResponse> {
        self.subscribe_with_meta(types, RequestMeta::default())
            .await
    }

    /// Subscribe to events with observability metadata.
    pub async fn subscribe_with_meta(
        &mut self,
        types: Vec<String>,
        meta: RequestMeta,
    ) -> Result<ControlResponse> {
        self.subscribe_with_meta_and_last_event_seq(types, meta, None)
            .await
    }

    /// Subscribe to events with observability metadata and reconnect cursor.
    pub async fn subscribe_with_meta_and_last_event_seq(
        &mut self,
        types: Vec<String>,
        meta: RequestMeta,
        last_event_seq: Option<i64>,
    ) -> Result<ControlResponse> {
        let params = match last_event_seq {
            Some(seq) => serde_json::json!({ "types": types, "last_event_seq": seq }),
            None => serde_json::json!({ "types": types }),
        };
        let request = ControlRequest::with_meta("events.subscribe", params, meta);
        let request_json = serde_json::to_string(&request)?;
        write_frame(&mut self.writer, &request_json)
            .await
            .context("sending subscribe request")?;

        let ack_json = read_frame(&mut self.reader, &mut self.buf)
            .await
            .context("reading subscribe ack")?;
        let ack: ControlResponse =
            serde_json::from_str(&ack_json).context("parsing subscribe ack")?;
        Ok(ack)
    }

    /// Receive the next event batch from a subscription stream.
    pub async fn recv_event_batch(&mut self) -> Result<EventSubscriptionBatchDto> {
        let batch_json = read_frame(&mut self.reader, &mut self.buf)
            .await
            .context("reading event batch")?;
        let batch: EventSubscriptionBatchDto =
            serde_json::from_str(&batch_json).context("parsing event batch")?;
        Ok(batch)
    }
}
