//! In-memory transport for protocol-layer tests. Cross-platform; runs on Linux CI.
//!
//! bind() returns a listener holding an mpsc::Receiver; connect() pushes one
//! end of a duplex pair through a globally-registered sender.
//!
//! The registry MUST be a global Mutex<HashMap>, not thread_local — tokio's
//! multi-threaded runtime may run bind() and connect() on different worker
//! threads, and a thread_local registry would make connect() fail to find
//! the endpoint.

use std::collections::HashMap;
use std::sync::OnceLock;

use anyhow::{Context, Result};
use async_trait::async_trait;
use tokio::io::DuplexStream;
use tokio::sync::{mpsc, Mutex};

use super::ControlTransport;

pub struct InMemoryTransport;

pub struct InMemoryListener {
    rx: std::sync::Arc<Mutex<mpsc::Receiver<DuplexStream>>>,
}

type Registry = Mutex<HashMap<String, mpsc::Sender<DuplexStream>>>;

static REGISTRY: OnceLock<Registry> = OnceLock::new();

fn registry() -> &'static Registry {
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

#[async_trait]
impl ControlTransport for InMemoryTransport {
    type Listener = InMemoryListener;
    type Stream = DuplexStream;

    async fn bind(endpoint: &str) -> Result<Self::Listener> {
        let (tx, rx) = mpsc::channel::<DuplexStream>(1);
        let mut guard = registry().lock().await;
        if guard.contains_key(endpoint) {
            anyhow::bail!("in-memory endpoint {endpoint} already bound");
        }
        guard.insert(endpoint.to_string(), tx);
        tracing::debug!(event_code = "control.in_memory.bound", endpoint = %endpoint);
        Ok(InMemoryListener { rx: std::sync::Arc::new(Mutex::new(rx)) })
    }

    async fn accept(listener: &Self::Listener) -> Result<Self::Stream> {
        listener.rx.lock().await.recv().await
            .context("in-memory listener closed before accept")
    }

    async fn connect(endpoint: &str) -> Result<Self::Stream> {
        let (client, server) = tokio::io::duplex(8 * 1024);
        let tx = {
            let guard = registry().lock().await;
            guard.get(endpoint).cloned()
                .with_context(|| format!("no in-memory listener for endpoint {endpoint}"))?
        };
        tx.send(server).await.context("listener dropped before connect")?;
        Ok(client)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unique_ep(label: &str) -> String {
        format!(
            "in-memory-{}-{}-{}",
            label,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        )
    }

    #[tokio::test]
    async fn roundtrip_through_duplex() {
        let ep = unique_ep("basic");
        let listener = InMemoryTransport::bind(&ep).await.unwrap();
        let accept_task = tokio::spawn(async move {
            InMemoryTransport::accept(&listener).await.unwrap()
        });
        let mut client = InMemoryTransport::connect(&ep).await.unwrap();
        let mut server = accept_task.await.unwrap();
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        client.write_all(b"hello").await.unwrap();
        let mut buf = [0u8; 5];
        server.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"hello");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn bind_and_connect_on_different_worker_threads() {
        // Regression: thread_local registry failed this; global registry works.
        let ep = unique_ep("cross-thread");
        let listener = InMemoryTransport::bind(&ep).await.unwrap();
        let ep_clone = ep.clone();
        let connect_task = tokio::spawn(async move {
            InMemoryTransport::connect(&ep_clone).await.unwrap()
        });
        let _client = connect_task.await.unwrap();
        let _server = InMemoryTransport::accept(&listener).await.unwrap();
    }
}
