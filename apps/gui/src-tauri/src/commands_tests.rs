use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::{
    fs,
    time::{SystemTime, UNIX_EPOCH},
};

use busytok_control::{
    server::ControlServer, transport::PlatformTransport, TestRuntimeControl,
};
use serde_json::json;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

use crate::host_application_services::invoke_busytok_via_socket_with_bootstrap;
use busytok_protocol::dto::RequestMeta;

#[tokio::test]
async fn tauri_invoke_proxies_methods_to_control_server() {
    let runtime = Arc::new(TestRuntimeControl::with_claude_fixture().await.unwrap());
    let (server, socket_path) =
        ControlServer::<PlatformTransport>::spawn_for_test(runtime).await.unwrap();
    let server = Arc::new(server);
    let server_task = {
        let server = Arc::clone(&server);
        tokio::spawn(async move { server.run().await })
    };

    for method in [
        "service.health",
        "service.status",
        "shell.status",
        "settings.snapshot",
        "settings.diagnostics",
    ] {
        let response = invoke_busytok_via_socket_with_bootstrap(
            method,
            json!({}),
            &socket_path,
            RequestMeta::default(),
            || async { Ok::<(), String>(()) },
        )
        .await;
        assert!(
            response.is_ok(),
            "{method} should return Ok payload: {response:?}"
        );
    }

    for (method, params) in [
        ("clients.snapshot", json!({})),
        ("activity.list", json!({"range": "day"})),
        (
            "breakdown.list",
            json!({"kind": "project", "range": "month"}),
        ),
        ("activity.detail", json!({"id": "test-id"})),
        ("clients.detail", json!({"source_id": "test-src"})),
        (
            "breakdown.detail",
            json!({"kind": "project", "id": "test-id", "range": "month"}),
        ),
    ] {
        let response = invoke_busytok_via_socket_with_bootstrap(
            method,
            params,
            &socket_path,
            RequestMeta::default(),
            || async { Ok::<(), String>(()) },
        )
        .await;
        if let Err(message) = &response {
            assert!(
                !message.contains("method_not_found"),
                "{method} should be routed, not method_not_found: {message}"
            );
        }
    }

    server.shutdown();
    server.await_drain().await;
    server_task.await.unwrap().unwrap();
}

#[tokio::test]
async fn tauri_invoke_returns_error_for_unknown_method() {
    let runtime = Arc::new(TestRuntimeControl::with_claude_fixture().await.unwrap());
    let (server, socket_path) =
        ControlServer::<PlatformTransport>::spawn_for_test(runtime).await.unwrap();
    let server = Arc::new(server);
    let server_task = {
        let server = Arc::clone(&server);
        tokio::spawn(async move { server.run().await })
    };

    let response = invoke_busytok_via_socket_with_bootstrap(
        "unknown.method",
        json!({}),
        &socket_path,
        RequestMeta::default(),
        || async { Ok::<(), String>(()) },
    )
    .await;
    assert!(response.is_err());
    assert!(response.unwrap_err().contains("method_not_found"));

    server.shutdown();
    server.await_drain().await;
    server_task.await.unwrap().unwrap();
}

#[tokio::test]
async fn tauri_invoke_bootstraps_service_when_socket_is_unavailable() {
    let tempdir = std::env::temp_dir().join(format!(
        "busytok-gui-test-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&tempdir).unwrap();
    let socket_path = tempdir.join("busytok.sock");
    let launched: Arc<Mutex<Option<(Arc<ControlServer>, JoinHandle<anyhow::Result<()>>)>>> =
        Arc::new(Mutex::new(None));

    let socket_str = socket_path.display().to_string();
    let response = invoke_busytok_via_socket_with_bootstrap(
        "shell.status",
        json!({}),
        &socket_str,
        RequestMeta {
            session_id: Some("test-session-001".into()),
            correlation_id: Some("test-correlation-abc".into()),
        },
        {
            let launched = Arc::clone(&launched);
            let socket_path = socket_path.clone();
            move || {
                let launched = Arc::clone(&launched);
                let socket_path = socket_path.clone();
                async move {
                    let runtime = Arc::new(
                        TestRuntimeControl::with_claude_fixture()
                            .await
                            .map_err(|e| e.to_string())?,
                    );
                    let server: Arc<ControlServer> = Arc::new(
                        ControlServer::bind(socket_path.display().to_string(), runtime)
                            .await
                            .map_err(|e| e.to_string())?,
                    );
                    let server_task = {
                        let server_for_task: Arc<ControlServer> = Arc::clone(&server);
                        tokio::spawn(async move { server_for_task.run().await })
                    };
                    *launched.lock().await = Some((server, server_task));
                    Ok(())
                }
            }
        },
    )
    .await;

    assert!(
        response.is_ok(),
        "invoke should succeed after bootstrap retry: {response:?}"
    );

    let (server, server_task) = launched
        .lock()
        .await
        .take()
        .expect("bootstrap callback should have launched a server");
    server.shutdown();
    server.await_drain().await;
    server_task.await.unwrap().unwrap();
    let _ = fs::remove_dir_all(&tempdir);
}

/// Verify that non-default `RequestMeta` survives the full
/// Tauri command → ControlServer → ControlDispatcher chain.
#[tokio::test]
async fn request_meta_survives_full_chain() {
    let runtime = Arc::new(TestRuntimeControl::with_claude_fixture().await.unwrap());
    let (server, socket_path) =
        ControlServer::<PlatformTransport>::spawn_for_test(Arc::clone(&runtime) as Arc<_>)
            .await
            .unwrap();
    let server = Arc::new(server);
    let server_task = {
        let server = Arc::clone(&server);
        tokio::spawn(async move { server.run().await })
    };

    let sp = socket_path.clone();
    invoke_busytok_via_socket_with_bootstrap(
        "shell.status",
        json!({}),
        &socket_path,
        RequestMeta {
            session_id: Some("sess-meta-test".into()),
            correlation_id: Some("corr-meta-test".into()),
        },
        move || {
            let _sp = sp.clone();
            async move { Ok::<(), String>(()) }
        },
    )
    .await
    .unwrap();

    let captured = runtime.last_meta.lock().unwrap().take();
    assert_eq!(
        captured.as_ref().and_then(|m| m.session_id.as_deref()),
        Some("sess-meta-test"),
    );
    assert_eq!(
        captured.as_ref().and_then(|m| m.correlation_id.as_deref()),
        Some("corr-meta-test"),
    );

    server.shutdown();
    server.await_drain().await;
    server_task.await.unwrap().unwrap();
}

#[tokio::test]
async fn tauri_invoke_serializes_concurrent_bootstrap_attempts() {
    let tempdir = std::env::temp_dir().join(format!(
        "busytok-gui-test-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&tempdir).unwrap();
    let socket_path = tempdir.join("busytok.sock");
    let launched: Arc<Mutex<Option<(Arc<ControlServer>, JoinHandle<anyhow::Result<()>>)>>> =
        Arc::new(Mutex::new(None));
    let bootstrap_calls = Arc::new(AtomicUsize::new(0));

    let make_call =
        |launched: Arc<Mutex<Option<(Arc<ControlServer>, JoinHandle<anyhow::Result<()>>)>>>,
         socket_path: std::path::PathBuf,
         bootstrap_calls: Arc<AtomicUsize>| async move {
            let socket_path_for_bootstrap = socket_path.clone();
            let socket_str = socket_path.display().to_string();
            invoke_busytok_via_socket_with_bootstrap(
                "shell.status",
                json!({}),
                &socket_str,
                RequestMeta::default(),
                move || {
                    let launched = Arc::clone(&launched);
                    let socket_path = socket_path_for_bootstrap.clone();
                    let bootstrap_calls = Arc::clone(&bootstrap_calls);
                    async move {
                        // NOTE: do NOT acquire `bootstrap_lock()` here.
                        // `connect_with_service_recovery` already holds
                        // the lock across this callback (I7 fix), which
                        // serializes bootstrap calls externally.
                        let previous = bootstrap_calls.fetch_add(1, Ordering::SeqCst);
                        if previous == 0 {
                            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                            let runtime = Arc::new(
                                TestRuntimeControl::with_claude_fixture()
                                    .await
                                    .map_err(|e| e.to_string())?,
                            );
                            let server: Arc<ControlServer> = Arc::new(
                                ControlServer::bind(socket_path.display().to_string(), runtime)
                                    .await
                                    .map_err(|e| e.to_string())?,
                            );
                            let server_task = {
                                let server_for_task: Arc<ControlServer> = Arc::clone(&server);
                                tokio::spawn(async move { server_for_task.run().await })
                            };
                            *launched.lock().await = Some((server, server_task));
                        }
                        Ok(())
                    }
                },
            )
            .await
        };

    let (first, second) = tokio::join!(
        make_call(
            Arc::clone(&launched),
            socket_path.clone(),
            Arc::clone(&bootstrap_calls)
        ),
        make_call(
            Arc::clone(&launched),
            socket_path.clone(),
            Arc::clone(&bootstrap_calls)
        ),
    );

    assert!(first.is_ok(), "first invoke should recover: {first:?}");
    assert!(second.is_ok(), "second invoke should recover: {second:?}");
    assert_eq!(
        bootstrap_calls.load(Ordering::SeqCst),
        1,
        "bootstrap should be serialized into a single recovery attempt"
    );

    let (server, server_task) = launched
        .lock()
        .await
        .take()
        .expect("bootstrap callback should have launched a server");
    server.shutdown();
    server.await_drain().await;
    server_task.await.unwrap().unwrap();
    let _ = fs::remove_dir_all(&tempdir);
}
