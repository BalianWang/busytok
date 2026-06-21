//! Verifies the protocol layer (frame, handshake, dispatch, subscribe)
//! works the same regardless of transport. Runs on any OS via InMemoryTransport.

use std::sync::Arc;

use busytok_control::{
    dispatch::{RuntimeControl, TestRuntimeControl},
    transport::in_memory::InMemoryTransport,
    ControlClient, ControlServer,
};
use busytok_protocol::dto::*;
use serde_json::json;

#[tokio::test]
async fn handshake_and_roundtrip_request() {
    let endpoint = format!(
        "protocol-test-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let runtime: Arc<dyn RuntimeControl> =
        Arc::new(TestRuntimeControl::with_claude_fixture().await.unwrap());
    let server =
        <ControlServer<InMemoryTransport>>::bind(&endpoint, runtime).await.unwrap();
    let server_task = tokio::spawn(async move {
        let _ = server.run().await;
    });

    let mut client =
        <ControlClient<InMemoryTransport>>::connect(&endpoint).await.unwrap();
    let req = ControlRequest::with_meta("service.health", json!({}), RequestMeta::default());
    let resp = client.call(req).await.unwrap();
    assert!(matches!(resp, ControlResponse::Ok(_)));

    server_task.abort();
}
