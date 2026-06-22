#![allow(
    clippy::await_holding_lock,
    clippy::too_many_arguments,
    clippy::type_complexity,
    clippy::field_reassign_with_default,
    clippy::uninlined_format_args,
    clippy::inconsistent_digit_grouping,
    clippy::len_zero,
    clippy::identity_op,
    clippy::useless_vec,
    clippy::manual_dangling_ptr,
    clippy::unwrap_used,
    clippy::unused_async,
    clippy::absurd_extreme_comparisons,
    unused_variables,
    unused_imports,
    dead_code
)]
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use busytok_runtime::read_service::{ReadErrorKind, ReadQuery, ReadService};
use busytok_store::Database;

#[derive(Clone, Default)]
struct SharedLogBuffer(Arc<Mutex<Vec<u8>>>);

impl SharedLogBuffer {
    fn clear(&self) {
        self.0.lock().unwrap().clear();
    }

    fn text(&self) -> String {
        String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
    }
}

struct SharedLogWriter(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for SharedLogWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for SharedLogBuffer {
    type Writer = SharedLogWriter;

    fn make_writer(&'a self) -> Self::Writer {
        SharedLogWriter(Arc::clone(&self.0))
    }
}

fn test_logs() -> SharedLogBuffer {
    static LOGS: OnceLock<SharedLogBuffer> = OnceLock::new();
    LOGS.get_or_init(SharedLogBuffer::default).clone()
}

fn init_test_logging() {
    static INIT: OnceLock<()> = OnceLock::new();
    INIT.get_or_init(|| {
        let subscriber = tracing_subscriber::fmt()
            .with_writer(test_logs())
            .with_ansi(false)
            .finish();
        let _ = tracing::subscriber::set_global_default(subscriber);
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn read_service_returns_structured_timeout() {
    let tmp = tempfile::TempDir::new().unwrap();
    let path = tmp.path().join("busytok.sqlite");
    let _db = Database::open(&path).unwrap();
    let service = ReadService::new(path, 2);

    let result = service
        .run(
            ReadQuery::new("test.timeout", "test_timeout").timeout(Duration::from_millis(1)),
            |_conn| {
                std::thread::sleep(Duration::from_millis(25));
                Ok::<_, anyhow::Error>(())
            },
        )
        .await;

    let err = result.unwrap_err();
    assert_eq!(err.kind(), ReadErrorKind::Timeout);
    assert_eq!(err.method(), "test.timeout");
    assert_eq!(err.query_family(), "test_timeout");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn read_service_limits_concurrency() {
    let tmp = tempfile::TempDir::new().unwrap();
    let path = tmp.path().join("busytok.sqlite");
    let _db = Database::open(&path).unwrap();
    let service = Arc::new(ReadService::new(path, 1));
    let (first_started_tx, first_started_rx) = std::sync::mpsc::channel();
    let second_started = Arc::new(AtomicBool::new(false));

    let first = {
        let service = Arc::clone(&service);
        tokio::spawn(async move {
            service
                .run(
                    ReadQuery::new("test.first", "test").timeout(Duration::from_secs(1)),
                    move |_conn| {
                        first_started_tx.send(()).unwrap();
                        std::thread::sleep(Duration::from_millis(50));
                        Ok::<_, anyhow::Error>(1)
                    },
                )
                .await
        })
    };
    first_started_rx
        .recv_timeout(Duration::from_secs(1))
        .unwrap();

    let second = {
        let service = Arc::clone(&service);
        let second_started = Arc::clone(&second_started);
        tokio::spawn(async move {
            service
                .run(
                    ReadQuery::new("test.second", "test").timeout(Duration::from_secs(1)),
                    move |_conn| {
                        second_started.store(true, Ordering::SeqCst);
                        Ok::<_, anyhow::Error>(2)
                    },
                )
                .await
        })
    };

    tokio::time::sleep(Duration::from_millis(10)).await;
    assert!(
        !second_started.load(Ordering::SeqCst),
        "second read should remain queued while the first read holds the only permit"
    );

    let (a, b) = tokio::join!(first, second);
    assert_eq!(a.unwrap().unwrap(), 1);
    assert_eq!(b.unwrap().unwrap(), 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn read_service_times_out_while_waiting_for_permit() {
    let tmp = tempfile::TempDir::new().unwrap();
    let path = tmp.path().join("busytok.sqlite");
    let _db = Database::open(&path).unwrap();
    let service = Arc::new(ReadService::new(path, 1));
    let (first_started_tx, first_started_rx) = std::sync::mpsc::channel();
    let second_started = Arc::new(AtomicBool::new(false));

    let first = {
        let service = Arc::clone(&service);
        tokio::spawn(async move {
            service
                .run(
                    ReadQuery::new("test.holder", "test_wait").timeout(Duration::from_secs(1)),
                    move |_conn| {
                        first_started_tx.send(()).unwrap();
                        std::thread::sleep(Duration::from_millis(75));
                        Ok::<_, anyhow::Error>(1)
                    },
                )
                .await
        })
    };
    first_started_rx
        .recv_timeout(Duration::from_secs(1))
        .unwrap();

    let err = service
        .run(
            ReadQuery::new("test.wait_timeout", "test_wait").timeout(Duration::from_millis(5)),
            {
                let second_started = Arc::clone(&second_started);
                move |_conn| {
                    second_started.store(true, Ordering::SeqCst);
                    Ok::<_, anyhow::Error>(2)
                }
            },
        )
        .await
        .unwrap_err();

    assert_eq!(err.kind(), ReadErrorKind::Timeout);
    assert_eq!(err.method(), "test.wait_timeout");
    assert_eq!(err.query_family(), "test_wait");
    assert!(
        !second_started.load(Ordering::SeqCst),
        "timed out request should never start executing its query closure"
    );

    assert_eq!(first.await.unwrap().unwrap(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn read_service_logs_timeout_without_completion_event() {
    init_test_logging();
    let logs = test_logs();
    logs.clear();

    let tmp = tempfile::TempDir::new().unwrap();
    let path = tmp.path().join("busytok.sqlite");
    let _db = Database::open(&path).unwrap();
    let service = ReadService::new(path, 1);

    let err = service
        .run(
            ReadQuery::new("test.timeout_log", "test_log").timeout(Duration::from_millis(1)),
            |_conn| {
                std::thread::sleep(Duration::from_millis(25));
                Ok::<_, anyhow::Error>(())
            },
        )
        .await
        .unwrap_err();

    assert_eq!(err.kind(), ReadErrorKind::Timeout);
    tokio::time::sleep(Duration::from_millis(50)).await;

    let lines: Vec<String> = logs.text().lines().map(str::to_owned).collect();
    assert!(
        lines.iter().any(|line| {
            line.contains("read.query.timed_out") && line.contains("method=\"test.timeout_log\"")
        }),
        "timeout should emit a read.query.timed_out event"
    );
    assert!(
        !lines.iter().any(|line| {
            line.contains("read.query.completed") && line.contains("method=\"test.timeout_log\"")
        }),
        "timed out requests must not emit read.query.completed"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn read_service_logs_completion_fields() {
    init_test_logging();
    let logs = test_logs();
    logs.clear();

    let tmp = tempfile::TempDir::new().unwrap();
    let path = tmp.path().join("busytok.sqlite");
    let _db = Database::open(&path).unwrap();
    let service = ReadService::new(path, 2);

    service
        .run(
            ReadQuery::new("test.success", "test")
                .generation_id_opt(Some("gen-1".to_string()))
                .readiness_opt(Some("ready_exact".to_string()))
                .watermark_ms_opt(Some(1234))
                .row_count(1)
                .used_read_model(true),
            |_conn| Ok::<_, anyhow::Error>(()),
        )
        .await
        .unwrap();

    let rendered = logs.text();
    assert!(rendered.contains("read.query.completed"));
    assert!(rendered.contains("method=\"test.success\""));
    assert!(rendered.contains("query_family=\"test\""));
    assert!(rendered.contains("generation_id=\"gen-1\""));
    assert!(rendered.contains("readiness=\"ready_exact\""));
    assert!(rendered.contains("watermark_ms=1234"));
    assert!(rendered.contains("row_count=1"));
    assert!(rendered.contains("used_read_model=true"));
    assert!(rendered.contains("status=\"ok\""));
}
