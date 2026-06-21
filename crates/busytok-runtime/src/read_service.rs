use std::fmt;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Result;
use rusqlite::ffi::ErrorCode;
use tokio::sync::Semaphore;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadErrorKind {
    Timeout,
    DatabaseBusy,
    Unavailable,
    Internal,
}

#[derive(Debug)]
pub struct ReadError {
    kind: ReadErrorKind,
    method: String,
    query_family: String,
    message: String,
}

impl ReadError {
    pub fn kind(&self) -> ReadErrorKind {
        self.kind
    }

    pub fn method(&self) -> &str {
        &self.method
    }

    pub fn query_family(&self) -> &str {
        &self.query_family
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    pub fn code(&self) -> &'static str {
        match self.kind {
            ReadErrorKind::Timeout => "read_timeout",
            ReadErrorKind::DatabaseBusy => "database_busy",
            ReadErrorKind::Unavailable => "read_model_unavailable",
            ReadErrorKind::Internal => "read_internal_error",
        }
    }
}

impl fmt::Display for ReadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code(), self.message)
    }
}

impl std::error::Error for ReadError {}

#[derive(Debug, Clone)]
pub struct ReadQuery {
    method: String,
    query_family: String,
    timeout: Duration,
    slow_after: Duration,
    generation_id: Option<String>,
    readiness: Option<String>,
    watermark_ms: Option<i64>,
    row_count: Option<usize>,
    used_read_model: bool,
}

pub struct ReadOutcome<T> {
    pub(crate) value: T,
    row_count: Option<usize>,
}

impl<T> ReadOutcome<T> {
    pub fn with_row_count(value: T, row_count: usize) -> Self {
        Self {
            value,
            row_count: Some(row_count),
        }
    }
}

impl<T> From<T> for ReadOutcome<T> {
    fn from(value: T) -> Self {
        Self {
            value,
            row_count: None,
        }
    }
}

impl ReadQuery {
    pub fn new(method: impl Into<String>, query_family: impl Into<String>) -> Self {
        Self {
            method: method.into(),
            query_family: query_family.into(),
            timeout: Duration::from_secs(2),
            slow_after: Duration::from_millis(100),
            generation_id: None,
            readiness: None,
            watermark_ms: None,
            row_count: None,
            used_read_model: false,
        }
    }

    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn slow_after(mut self, slow_after: Duration) -> Self {
        self.slow_after = slow_after;
        self
    }

    pub fn generation_id_opt(mut self, generation_id: Option<String>) -> Self {
        self.generation_id = generation_id;
        self
    }

    pub fn readiness_opt(mut self, readiness: Option<String>) -> Self {
        self.readiness = readiness;
        self
    }

    pub fn watermark_ms_opt(mut self, watermark_ms: Option<i64>) -> Self {
        self.watermark_ms = watermark_ms;
        self
    }

    pub fn row_count(mut self, row_count: usize) -> Self {
        self.row_count = Some(row_count);
        self
    }

    pub fn used_read_model(mut self, used_read_model: bool) -> Self {
        self.used_read_model = used_read_model;
        self
    }
}

enum ReadBackend {
    File {
        db_path: PathBuf,
        idle: Arc<Mutex<Vec<busytok_store::Database>>>,
        max_pool_size: usize,
    },
    Memory {
        db: Arc<Mutex<busytok_store::Database>>,
    },
}

pub struct ReadService {
    backend: ReadBackend,
    permits: Arc<Semaphore>,
}

impl ReadService {
    pub fn new(db_path: PathBuf, max_connections: usize) -> Self {
        assert!(
            max_connections > 0,
            "max_connections must be greater than zero"
        );

        Self {
            backend: ReadBackend::File {
                db_path,
                idle: Arc::new(Mutex::new(Vec::with_capacity(max_connections))),
                max_pool_size: max_connections,
            },
            permits: Arc::new(Semaphore::new(max_connections)),
        }
    }

    pub fn new_in_memory(db: Arc<Mutex<busytok_store::Database>>, max_connections: usize) -> Self {
        assert!(
            max_connections > 0,
            "max_connections must be greater than zero"
        );

        Self {
            backend: ReadBackend::Memory { db },
            permits: Arc::new(Semaphore::new(max_connections)),
        }
    }

    pub async fn run<T, R, F>(&self, query: ReadQuery, f: F) -> Result<T, ReadError>
    where
        T: Send + 'static,
        R: Into<ReadOutcome<T>> + Send + 'static,
        F: FnOnce(&rusqlite::Connection) -> Result<R> + Send + 'static,
    {
        let started = Instant::now();
        let deadline = tokio::time::Instant::now() + query.timeout;
        let timeout_ms = duration_ms_u64(query.timeout);
        let slow_after_ms = duration_ms_u64(query.slow_after);
        let permit =
            match tokio::time::timeout_at(deadline, self.permits.clone().acquire_owned()).await {
                Ok(Ok(permit)) => permit,
                Ok(Err(_)) => {
                    let err = unavailable_error(&query, "read service closed");
                    log_completion(
                        &query,
                        timeout_ms,
                        duration_ms_u64(started.elapsed()),
                        slow_after_ms,
                        None,
                        Some(&err),
                    );
                    return Err(err);
                }
                Err(_) => {
                    let err = timeout_error(&query);
                    log_timeout(
                        &query,
                        timeout_ms,
                        duration_ms_u64(started.elapsed()),
                        slow_after_ms,
                    );
                    return Err(err);
                }
            };

        if tokio::time::Instant::now() >= deadline {
            let err = timeout_error(&query);
            log_timeout(
                &query,
                timeout_ms,
                duration_ms_u64(started.elapsed()),
                slow_after_ms,
            );
            return Err(err);
        }

        let query_for_task = query.clone();

        let task = match &self.backend {
            ReadBackend::File {
                db_path,
                idle,
                max_pool_size,
            } => {
                let db_path = db_path.clone();
                let idle = Arc::clone(idle);
                let max_pool_size = *max_pool_size;
                tokio::task::spawn_blocking(move || {
                    let _permit = permit;
                    let db = take_connection(&idle, &db_path, &query_for_task)?;
                    let result = f(db.conn())
                        .map(Into::into)
                        .map_err(|err| map_read_error(&query_for_task, err));
                    return_connection(&idle, db, max_pool_size);
                    result
                })
            }
            ReadBackend::Memory { db } => {
                let db = Arc::clone(db);
                tokio::task::spawn_blocking(move || {
                    let _permit = permit;
                    let guard = db.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                    f(guard.conn())
                        .map(Into::into)
                        .map_err(|err| map_read_error(&query_for_task, err))
                })
            }
        };

        // `timeout_at` covers both permit wait and blocking execution with one
        // absolute deadline. If the deadline is reached before we spawn the
        // blocking task, no detached work remains. Once the blocking task has
        // started, however, timing out only stops awaiting its result: the task
        // keeps running and retains its semaphore permit/connection until the
        // SQLite closure returns. We accept that tradeoff to keep the design
        // simple and predictable; callers must keep read queries bounded.
        match tokio::time::timeout_at(deadline, task).await {
            Ok(Ok(Ok(outcome))) => {
                log_completion(
                    &query,
                    timeout_ms,
                    duration_ms_u64(started.elapsed()),
                    slow_after_ms,
                    outcome.row_count,
                    None,
                );
                Ok(outcome.value)
            }
            Ok(Ok(Err(err))) => {
                log_completion(
                    &query,
                    timeout_ms,
                    duration_ms_u64(started.elapsed()),
                    slow_after_ms,
                    query.row_count,
                    Some(&err),
                );
                Err(err)
            }
            Ok(Err(join_err)) => {
                let err = internal_error(&query, format!("spawn_blocking join error: {join_err}"));
                log_completion(
                    &query,
                    timeout_ms,
                    duration_ms_u64(started.elapsed()),
                    slow_after_ms,
                    None,
                    Some(&err),
                );
                Err(err)
            }
            Err(_) => {
                let err = timeout_error(&query);
                log_timeout(
                    &query,
                    timeout_ms,
                    duration_ms_u64(started.elapsed()),
                    slow_after_ms,
                );
                Err(err)
            }
        }
    }
}

fn take_connection(
    idle: &Mutex<Vec<busytok_store::Database>>,
    db_path: &PathBuf,
    query: &ReadQuery,
) -> Result<busytok_store::Database, ReadError> {
    if let Some(db) = idle_guard(idle).pop() {
        return Ok(db);
    }

    busytok_store::Database::open_readonly(db_path).map_err(|err| map_open_error(query, err))
}

fn return_connection(
    idle: &Mutex<Vec<busytok_store::Database>>,
    db: busytok_store::Database,
    max_pool_size: usize,
) {
    let mut pool = idle_guard(idle);
    if pool.len() < max_pool_size {
        pool.push(db);
    }
}

fn idle_guard(
    idle: &Mutex<Vec<busytok_store::Database>>,
) -> std::sync::MutexGuard<'_, Vec<busytok_store::Database>> {
    idle.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn map_open_error(query: &ReadQuery, err: anyhow::Error) -> ReadError {
    let message = err.to_string();
    let kind = match sqlite_error_code(&err) {
        Some(ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked) => ReadErrorKind::DatabaseBusy,
        Some(
            ErrorCode::CannotOpen
            | ErrorCode::NotFound
            | ErrorCode::NotADatabase
            | ErrorCode::PermissionDenied,
        ) => ReadErrorKind::Unavailable,
        _ => ReadErrorKind::Internal,
    };

    ReadError {
        kind,
        method: query.method.clone(),
        query_family: query.query_family.clone(),
        message,
    }
}

fn map_read_error(query: &ReadQuery, err: anyhow::Error) -> ReadError {
    let message = err.to_string();
    let kind = match sqlite_error_code(&err) {
        Some(ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked) => ReadErrorKind::DatabaseBusy,
        Some(ErrorCode::CannotOpen | ErrorCode::NotFound | ErrorCode::NotADatabase) => {
            ReadErrorKind::Unavailable
        }
        _ if message.contains("database is locked") || message.contains("database is busy") => {
            ReadErrorKind::DatabaseBusy
        }
        _ => ReadErrorKind::Internal,
    };

    ReadError {
        kind,
        method: query.method.clone(),
        query_family: query.query_family.clone(),
        message,
    }
}

fn sqlite_error_code(err: &anyhow::Error) -> Option<ErrorCode> {
    err.chain()
        .find_map(|cause| match cause.downcast_ref::<rusqlite::Error>() {
            Some(rusqlite::Error::SqliteFailure(sql_err, _)) => Some(sql_err.code),
            _ => None,
        })
}

fn timeout_error(query: &ReadQuery) -> ReadError {
    ReadError {
        kind: ReadErrorKind::Timeout,
        method: query.method.clone(),
        query_family: query.query_family.clone(),
        message: format!(
            "read query timed out after {} ms",
            duration_ms_u64(query.timeout)
        ),
    }
}

fn unavailable_error(query: &ReadQuery, message: impl Into<String>) -> ReadError {
    ReadError {
        kind: ReadErrorKind::Unavailable,
        method: query.method.clone(),
        query_family: query.query_family.clone(),
        message: message.into(),
    }
}

fn internal_error(query: &ReadQuery, message: impl Into<String>) -> ReadError {
    ReadError {
        kind: ReadErrorKind::Internal,
        method: query.method.clone(),
        query_family: query.query_family.clone(),
        message: message.into(),
    }
}

fn duration_ms_u64(duration: Duration) -> u64 {
    duration.as_millis().try_into().unwrap_or(u64::MAX)
}

fn log_completion(
    query: &ReadQuery,
    timeout_ms: u64,
    elapsed_ms: u64,
    slow_after_ms: u64,
    row_count: Option<usize>,
    error: Option<&ReadError>,
) {
    let slow = elapsed_ms >= slow_after_ms;
    let (status, error_code) = match error {
        Some(err) => ("error", err.code()),
        None => ("ok", ""),
    };

    tracing::info!(
        method = query.method.as_str(),
        query_family = query.query_family.as_str(),
        generation_id = query.generation_id.as_deref().unwrap_or(""),
        readiness = query.readiness.as_deref().unwrap_or(""),
        watermark_ms = query.watermark_ms.unwrap_or_default(),
        row_count = row_count.or(query.row_count).unwrap_or_default(),
        used_read_model = query.used_read_model,
        elapsed_ms,
        slow,
        timeout_ms,
        status,
        error_code,
        "read.query.completed"
    );
}

fn log_timeout(query: &ReadQuery, timeout_ms: u64, elapsed_ms: u64, slow_after_ms: u64) {
    let slow = elapsed_ms >= slow_after_ms;
    tracing::info!(
        method = query.method.as_str(),
        query_family = query.query_family.as_str(),
        generation_id = query.generation_id.as_deref().unwrap_or(""),
        readiness = query.readiness.as_deref().unwrap_or(""),
        watermark_ms = query.watermark_ms.unwrap_or_default(),
        row_count = query.row_count.unwrap_or_default(),
        used_read_model = query.used_read_model,
        elapsed_ms,
        timeout_ms,
        slow,
        "read.query.timed_out"
    );
}
