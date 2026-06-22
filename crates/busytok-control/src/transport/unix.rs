//! Unix-domain-socket transport.

use std::path::Path;

use anyhow::{Context, Result};
use async_trait::async_trait;
use tokio::net::{UnixListener, UnixStream};

use super::ControlTransport;

pub struct UnixTransport;

#[async_trait]
impl ControlTransport for UnixTransport {
    type Listener = UnixListener;
    type Stream = UnixStream;

    async fn bind(endpoint: &str) -> Result<Self::Listener> {
        let path = Path::new(endpoint);
        if path.exists() {
            std::fs::remove_file(path).context("removing stale socket")?;
        }
        let listener = UnixListener::bind(path).context("binding Unix domain socket")?;
        tracing::info!(event_code = "control.transport.unix.bound", endpoint = %endpoint);
        Ok(listener)
    }

    async fn accept(listener: &Self::Listener) -> Result<Self::Stream> {
        let (stream, _addr) = listener
            .accept()
            .await
            .context("accepting Unix connection")?;
        Ok(stream)
    }

    async fn connect(endpoint: &str) -> Result<Self::Stream> {
        UnixStream::connect(endpoint)
            .await
            .with_context(|| format!("connecting to control endpoint {endpoint}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn bind_accept_connect_roundtrip() {
        let dir = tempdir().unwrap();
        let endpoint = dir.path().join("test.sock");
        let endpoint_str = endpoint.display().to_string();

        let listener = UnixTransport::bind(&endpoint_str).await.unwrap();
        let accept_task =
            tokio::spawn(async move { UnixTransport::accept(&listener).await.unwrap() });
        let _client = UnixTransport::connect(&endpoint_str).await.unwrap();
        let _server_stream = accept_task.await.unwrap();
    }

    #[tokio::test]
    async fn bind_removes_stale_socket() {
        let dir = tempdir().unwrap();
        let endpoint = dir.path().join("stale.sock");
        std::fs::write(&endpoint, b"").unwrap();
        assert!(endpoint.exists());

        let _listener = UnixTransport::bind(&endpoint.display().to_string())
            .await
            .unwrap();
        assert!(endpoint.exists()); // socket is re-created, note: it IS a file on Unix
    }
}
