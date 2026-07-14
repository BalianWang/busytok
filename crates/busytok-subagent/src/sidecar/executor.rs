use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use tracing::{error, info, warn};

use busytok_store::{Database, SubagentHarnessBindingRow, SubagentResourceEventRow};

use crate::error::SubagentError;
use crate::memory::MemoryUpdate;
use crate::mock_executor::{ExecutorInput, ExecutorOutput, TaskExecutor};
use crate::models::{TaskErrorKind, TaskStatus, TaskUsage};
use crate::sidecar::pool::WorkerPool;
use crate::sidecar::protocol::{
    AUTH_FAILURE, HOT_SESSION_LIMIT_REACHED, NETWORK_ERROR, RATE_LIMIT, SESSION_NOT_FOUND,
    TASK_TIMEOUT,
};
use crate::sidecar::SidecarError;

/// Drives a single subagent turn through the Pi sidecar.
///
/// Phase 3 Task 3: rewired from a single `Arc<PiSidecarSupervisor>` to
/// `Arc<WorkerPool>`. The `execute()` method reads `input.provider_id`,
/// calls `pool.ensure_worker(provider_id)` to get the supervisor, then
/// delegates to it. On auth failure (`TaskErrorKind::Auth`), the worker is
/// hard-killed + removed via `pool.remove_worker_and_kill(provider_id)`
/// (bypasses the 5-min restart window — the credential is bad, restarting
/// won't help).
///
/// On `HOT_SESSION_LIMIT_REACHED` from `turn_auto`, the executor catches the
/// error, extracts the LRU `candidate` from the RPC error's `data.candidate`
/// field (the sidecar is the hot-pool authority — spec §4.4), drives eviction
/// (`prepare_hibernate` → atomic persist → `close`), and retries `turn_auto`
/// exactly once.
///
/// `session.close` failure during eviction is **FATAL**: the DB has already
/// been flipped (binding `is_hot=0`, logical status `warm`), but the sidecar
/// still holds the session hot. Retrying `turn_auto` would hit
/// `HOT_SESSION_LIMIT_REACHED` again and the sidecar/DB would diverge. We
/// propagate the error so the caller knows a sidecar restart is the recovery
/// path.
pub struct SidecarTaskExecutor {
    pool: Arc<WorkerPool>,
    db: Option<Arc<Mutex<Database>>>,
}

/// Outcome of an eviction attempt (Bug 1 concurrent eviction fix).
///
/// When two concurrent delegates both receive the same LRU candidate from
/// the sidecar, both call `evict_session`. The first evictor succeeds
/// (`Evicted`); the second finds the DB binding already flipped to
/// `is_hot=0` by the first — this is NOT an error, just `AlreadyEvicted`.
/// The caller retries `turn_auto` in both cases. For `Evicted`, the slot
/// is already free (close completed). For `AlreadyEvicted`, the concurrent
/// evictor's `close` may not have completed yet — the caller uses bounded
/// retry with backoff to wait for the slot to free up.
#[derive(Debug)]
enum EvictionOutcome {
    /// We successfully drove the full eviction flow (prepare → DB commit → close).
    Evicted,
    /// Another concurrent evictor already flipped the DB binding to `is_hot=0`.
    /// The session `close` may or may not have completed yet — the caller must
    /// retry `turn_auto` with bounded backoff to wait for the slot to free.
    AlreadyEvicted,
}

/// Max retry attempts for `turn_auto` after `AlreadyEvicted`. The concurrent
/// evictor's `close` RPC is fast (just removes the session from the sidecar
/// pool), so a small number of retries with short backoff is sufficient.
const EVICT_WAIT_MAX_RETRIES: u32 = 5;

/// Backoff between retries after `AlreadyEvicted`. 20ms × 5 retries = 100ms
/// max wait — enough for a concurrent `close` RPC to complete.
const EVICT_WAIT_BACKOFF: std::time::Duration = std::time::Duration::from_millis(20);

/// Max retry attempts for `turn_auto` when all hot sessions are busy
/// (`HotLimitOutcome::AllBusy`). This is transient capacity contention —
/// a running task in a different subagent is holding the only hot session
/// slot. We retry with backoff instead of immediately failing the task.
/// 10 retries × 500ms = 5s max wait — enough for short tasks to complete
/// and free the slot. Longer contention is handled by re-queuing the task
/// in `execute_and_persist` (see `HOT_SESSION_RETRY_DEADLINE_MS`).
const ALL_BUSY_MAX_RETRIES: u32 = 10;

/// Backoff between retries when all hot sessions are busy. 500ms provides
/// reasonable spacing without excessive latency for short tasks.
const ALL_BUSY_BACKOFF: std::time::Duration = std::time::Duration::from_millis(500);

impl SidecarTaskExecutor {
    /// Construct with a `WorkerPool` + optional DB handle (production path).
    ///
    /// The pool routes `execute()` calls to the correct per-provider
    /// supervisor via `ensure_worker(provider_id)`. The DB is used by the
    /// eviction flow (`evict_lru` / `evict_session`) to persist memory deltas
    /// and flip bindings atomically. Without a DB the executor cannot persist
    /// memory deltas or flip bindings, so `HOT_SESSION_LIMIT_REACHED`
    /// surfaces as a fatal error: `evict_session` rejects the no-DB path up
    /// front (before any RPC) to avoid silent state divergence — the sidecar
    /// would release its slot on `close` but the DB would still believe the
    /// session is hot.
    pub fn with_pool(pool: Arc<WorkerPool>, db: Option<Arc<Mutex<Database>>>) -> Self {
        Self { pool, db }
    }

    /// Access the underlying pool (used by wiring + tests).
    pub fn pool(&self) -> &Arc<WorkerPool> {
        &self.pool
    }

    /// Best-effort cancel: send `session.cancel` RPC to the sidecar process
    /// that owns the subagent's hot session. The sidecar calls `abort()` on
    /// the SDK session, which aborts the in-flight HTTP request to the LLM
    /// provider — stopping token generation.
    ///
    /// If no worker exists for `provider_id` (sidecar was never started or
    /// was killed), returns `Ok(())` — there's nothing to cancel. If the
    /// sidecar is unreachable or the RPC fails, the error is returned so
    /// the caller can log it. The cancel outcome in the DB is NOT affected
    /// by this call's result — the DB status is already `cancelled`.
    async fn cancel_turn(
        &self,
        subagent_id: &str,
        provider_id: &str,
        task_id: Option<&str>,
    ) -> anyhow::Result<()> {
        let Some(supervisor) = self.pool.get_worker(provider_id) else {
            // No worker — sidecar was never started or was killed.
            // Nothing to cancel.
            return Ok(());
        };
        // Don't spawn a sidecar just to cancel — if the process died, its
        // hot sessions died with it and there's nothing to abort.
        if !supervisor.try_is_running() {
            return Ok(());
        }
        let handle = supervisor
            .ensure_started()
            .await
            .map_err(|e| anyhow::anyhow!("sidecar ensure_started failed during cancel: {e}"))?;
        handle
            .cancel_session(subagent_id, task_id)
            .await
            .map_err(|e| anyhow::anyhow!("session.cancel RPC failed: {e}"))?;
        Ok(())
    }

    /// Activate a session — move it from `pending` to `active` (LRU-eligible)
    /// in the sidecar's hot pool. Called by the manager AFTER the DB hot
    /// binding is committed. On failure (RPC timeout, SESSION_NOT_FOUND,
    /// sidecar crash), the manager rolls back the DB binding to warm/cold
    /// and closes the pending session — the task already completed, so the
    /// user's result is preserved; future delegates will cold-start.
    async fn activate_session_rpc(
        &self,
        adapter_session_id: &str,
        provider_id: &str,
    ) -> anyhow::Result<()> {
        let Some(supervisor) = self.pool.get_worker(provider_id) else {
            warn!(
                event_code = "subagent.session.activate_worker_missing",
                adapter_session_id = %adapter_session_id,
                provider_id = %provider_id,
                "cannot activate session: provider worker is missing"
            );
            return Err(anyhow::anyhow!(
                "provider worker {provider_id} is missing while activating session {adapter_session_id}"
            ));
        };
        let handle = supervisor
            .ensure_started()
            .await
            .map_err(|e| anyhow::anyhow!("sidecar ensure_started failed during activate: {e}"))?;
        handle
            .activate(adapter_session_id)
            .await
            .map_err(|e| anyhow::anyhow!("session.activate RPC failed: {e}"))?;
        Ok(())
    }

    /// Close a session after a DB lifecycle transition. The operation is
    /// idempotent: a session that was already removed by the sidecar (or a
    /// concurrent cleanup) is treated as successfully closed.
    async fn close_session_rpc(
        &self,
        adapter_session_id: &str,
        provider_id: &str,
    ) -> anyhow::Result<()> {
        let Some(supervisor) = self.pool.get_worker(provider_id) else {
            return Ok(());
        };
        if !supervisor.try_is_running() {
            return Ok(());
        }
        let handle = supervisor
            .ensure_started()
            .await
            .map_err(|e| anyhow::anyhow!("sidecar ensure_started failed during close: {e}"))?;
        match handle.close(adapter_session_id).await {
            Ok(_) => {}
            Err(SidecarError::Application(code, _message, _data)) if code == SESSION_NOT_FOUND => {
                info!(
                    event_code = "subagent.session.close_already_closed",
                    adapter_session_id = %adapter_session_id,
                    provider_id = %provider_id,
                    "session.close received SESSION_NOT_FOUND; treating cleanup as complete"
                );
            }
            Err(e) => return Err(anyhow::anyhow!("session.close RPC failed: {e}")),
        }
        Ok(())
    }
}

#[async_trait]
impl TaskExecutor for SidecarTaskExecutor {
    async fn execute(&self, input: &ExecutorInput) -> anyhow::Result<ExecutorOutput> {
        Self::execute_impl(self, input).await
    }

    async fn cancel_for_task(
        &self,
        subagent_id: &str,
        provider_id: &str,
        task_id: &str,
    ) -> anyhow::Result<()> {
        self.cancel_turn(subagent_id, provider_id, Some(task_id))
            .await
    }

    async fn cancel(&self, subagent_id: &str, provider_id: &str) -> anyhow::Result<()> {
        self.cancel_turn(subagent_id, provider_id, None).await
    }

    async fn activate_session(
        &self,
        adapter_session_id: &str,
        provider_id: &str,
    ) -> anyhow::Result<()> {
        self.activate_session_rpc(adapter_session_id, provider_id)
            .await
    }

    async fn close_session(
        &self,
        adapter_session_id: &str,
        provider_id: &str,
    ) -> anyhow::Result<()> {
        self.close_session_rpc(adapter_session_id, provider_id)
            .await
    }
}

impl SidecarTaskExecutor {
    async fn execute_impl(&self, input: &ExecutorInput) -> anyhow::Result<ExecutorOutput> {
        // Task 5: provider_id is now String (always present, validated upstream
        // by `SubagentManager::execute_task`). No "profile not bound" branch.
        let provider_id = input.provider_id.clone();

        // Step 2: ensure_worker (synchronous — I2 fix).
        let supervisor = self
            .pool
            .ensure_worker(&provider_id)
            .map_err(sidecar_to_anyhow)?;

        // Step 3: ensure_started (async — lazy spawn if needed).
        let handle = supervisor
            .ensure_started()
            .await
            .map_err(sidecar_to_anyhow)?;

        // Step 4: build turn_auto params (spec §4.3). Include provider_id
        // so the sidecar knows which provider to use.
        let key_files_json: Vec<serde_json::Value> = input
            .memory
            .key_files
            .iter()
            .map(|f| {
                serde_json::json!({
                    "path": f.path,
                    "reason": f.reason,
                    "last_seen_at_ms": f.last_seen_at_ms,
                    "score": f.score,
                })
            })
            .collect();
        let open_questions_json: Vec<serde_json::Value> = input
            .memory
            .open_questions
            .iter()
            .map(|q| {
                serde_json::json!({
                    "question": q.question,
                    "status": q.status,
                    "created_at_ms": q.created_at_ms,
                    "last_seen_at_ms": q.last_seen_at_ms,
                })
            })
            .collect();
        let memory_json = serde_json::json!({
            "hot_summary": input.memory.hot_summary,
            "long_summary": input.memory.long_summary,
            "key_files": key_files_json,
            "decisions": input.memory.decisions,
            "open_questions": open_questions_json,
        });
        let params = serde_json::json!({
            "task_id": input.task_id,
            "logical_subagent_id": input.subagent_id,
            "logical_subagent_name": input.subagent_name,
            "cwd": input.cwd,
            "profile": input.profile,
            "model": input.model,
            "tools": input.tools,
            "prompt": input.prompt,
            "prompt_artifact_ref": input.prompt_artifact_ref,
            "memory": memory_json,
            "context": {
                "compact_context": input.context.compact_context,
                "budget_tokens": input.context.budget_tokens,
                "source": input.context.source,
            },
            "timeout_ms": input.timeout_seconds.map(|s| s * 1000),
            "constraints": {
                "write_access": input.write_access,
                "timeout_ms": input.timeout_seconds.map(|s| s * 1000).unwrap_or(180000),
            },
            "output_schema": {
                "format": "json",
                "name": "review_result",
                "version": 1,
            },
            "adapter_options": {},
            "provider_id": provider_id,
            "provider_kind": input.provider_kind,
            "provider_base_url": input.provider_base_url,
            // provider_api_key is sent so the sidecar can register it in
            // AuthStorage. Sidecar must NOT log this field in plaintext
            // (Task 7 enforces).
            "provider_api_key": input.provider_api_key,
            "model_reasoning": input.model_reasoning,
            "model_context_window": input.model_context_window,
            "model_max_tokens": input.model_max_tokens,
            "model_display_name": input.model_display_name,
        });
        info!(
            event_code = "subagent.sidecar.turn_auto.start",
            subagent_id = %input.subagent_id,
            profile = %input.profile,
            provider_id = %provider_id,
            "sending turn_auto to sidecar"
        );
        // Step 5-6: call turn_auto, classify errors, auth-fail kill.
        match handle.turn_auto(params.clone()).await {
            Ok(result) => Ok(parse_turn_auto_result(&result)),
            Err(SidecarError::Application(code, _msg, data))
                if code == HOT_SESSION_LIMIT_REACHED =>
            {
                // Bug 1 fix: the sidecar returns `data.candidate = null`
                // + `data.all_busy = true` when all hot sessions are
                // in-use (busy). In that case, skip eviction entirely —
                // there is no safe candidate to evict. Surface a
                // `HotSessionLimit` error so the task fails with a clear
                // message instead of a doomed eviction attempt +
                // "database error" mislabel (Bug 2).
                match classify_hot_limit_error(data.as_ref()) {
                    HotLimitOutcome::Evict(c) => {
                        info!(
                            event_code = "subagent.session.hot_limit_reached",
                            subagent_id = %input.subagent_id,
                            candidate = %c,
                            "hot session limit reached, driving eviction"
                        );
                        let outcome = self.evict_session(&c).await?;
                        // Retry turn_auto after eviction. Both `Evicted` and
                        // `AlreadyEvicted` use bounded retry with backoff:
                        //
                        // - `Evicted`: we successfully closed the LRU session,
                        //   but a concurrent delegate may have grabbed the
                        //   freed slot before our retry. HOT_SESSION_LIMIT_REACHED
                        //   on retry is transient capacity contention — retry
                        //   with backoff instead of failing.
                        // - `AlreadyEvicted`: another evictor flipped the DB
                        //   binding but their `close` may not have completed
                        //   yet. Bounded retry with backoff until the slot
                        //   frees up.
                        //
                        // In both cases, HOT_SESSION_LIMIT_REACHED during the
                        // retry window is treated as transient and retried
                        // (NOT propagated to the generic failed_after_eviction
                        // path). After EVICT_WAIT_MAX_RETRIES, the executor
                        // surfaces `SubagentError::HotSessionLimit` so
                        // `execute_and_persist` can re-queue the task.
                        let max_attempts = EVICT_WAIT_MAX_RETRIES;
                        for attempt in 0..max_attempts {
                            if attempt > 0 {
                                tokio::time::sleep(EVICT_WAIT_BACKOFF).await;
                            }
                            match handle.turn_auto(params.clone()).await {
                                Ok(result) => return Ok(parse_turn_auto_result(&result)),
                                Err(SidecarError::Application(code, _msg, _data))
                                    if code == HOT_SESSION_LIMIT_REACHED =>
                                {
                                    info!(
                                        event_code = "subagent.session.eviction_wait_retry",
                                        subagent_id = %input.subagent_id,
                                        attempt = attempt + 1,
                                        max_attempts,
                                        outcome = ?outcome,
                                        "turn_auto hit HOT_SESSION_LIMIT after eviction — waiting for slot to free"
                                    );
                                    continue;
                                }
                                Err(e) => {
                                    let kind = classify_sidecar_error(&e);
                                    warn!(
                                        event_code = "subagent.sidecar.turn_auto.failed_after_eviction",
                                        error = %e,
                                        error_kind = ?kind
                                    );
                                    if kind == TaskErrorKind::Auth {
                                        if let Err(kill_err) =
                                            self.pool.remove_worker_and_kill(&provider_id).await
                                        {
                                            error!(
                                                event_code = "subagent.pool.auth_kill_failed",
                                                provider_id = %provider_id,
                                                error = %kill_err,
                                                "remove_worker_and_kill failed after auth error"
                                            );
                                        }
                                    }
                                    return Err(sidecar_to_anyhow(e));
                                }
                            }
                        }
                        // Reached when all EVICT_WAIT_MAX_RETRIES retries are
                        // exhausted — the slot never freed within the retry
                        // window (concurrent task holds it, or the evictor's
                        // close never completed). Surface as HotSessionLimit so
                        // `execute_and_persist` can re-queue the task for
                        // dispatcher retry (bounded by HOT_SESSION_RETRY_DEADLINE_MS).
                        warn!(
                            event_code = "subagent.session.eviction_wait_exhausted",
                            subagent_id = %input.subagent_id,
                            retries = EVICT_WAIT_MAX_RETRIES,
                            outcome = ?outcome,
                            "exhausted retries waiting for slot to free — surfacing as HotSessionLimit"
                        );
                        Err(anyhow::Error::from(SubagentError::HotSessionLimit {
                            candidate: String::new(),
                        }))
                    }
                    HotLimitOutcome::AllBusy => {
                        // Transient capacity contention: all hot sessions are
                        // in-use by running tasks. The slot will free up when
                        // one of them completes. Retry with backoff instead of
                        // immediately failing the task.
                        //
                        // This is the fix for the P0 bug where two different
                        // subagents competing for the global hot session limit
                        // caused one to fail with "hot session limit reached"
                        // instead of waiting for the slot to free up.
                        info!(
                            event_code = "subagent.session.hot_limit_all_busy",
                            subagent_id = %input.subagent_id,
                            "hot session limit reached — all sessions busy, retrying with backoff"
                        );
                        for attempt in 0..ALL_BUSY_MAX_RETRIES {
                            tokio::time::sleep(ALL_BUSY_BACKOFF).await;
                            match handle.turn_auto(params.clone()).await {
                                Ok(result) => {
                                    info!(
                                        event_code = "subagent.session.hot_limit_all_busy_recovered",
                                        subagent_id = %input.subagent_id,
                                        attempt = attempt + 1,
                                        "turn_auto succeeded after all-busy retry — slot freed"
                                    );
                                    return Ok(parse_turn_auto_result(&result));
                                }
                                Err(SidecarError::Application(code, _msg, data))
                                    if code == HOT_SESSION_LIMIT_REACHED =>
                                {
                                    match classify_hot_limit_error(data.as_ref()) {
                                        HotLimitOutcome::AllBusy => {
                                            info!(
                                                event_code = "subagent.session.hot_limit_all_busy_retry",
                                                subagent_id = %input.subagent_id,
                                                attempt = attempt + 1,
                                                max_attempts = ALL_BUSY_MAX_RETRIES,
                                                "all hot sessions still busy — retrying"
                                            );
                                            continue;
                                        }
                                        // A candidate surfaced during AllBusy
                                        // retry — fall through to surface the
                                        // error. Eviction mid-AllBusy is
                                        // unexpected (the slot is busy, not
                                        // evictable) but we don't want to loop
                                        // forever on an unexpected state
                                        // transition.
                                        HotLimitOutcome::Evict(_)
                                        | HotLimitOutcome::ProtocolViolation(_) => {
                                            break;
                                        }
                                    }
                                }
                                Err(e) => {
                                    let kind = classify_sidecar_error(&e);
                                    warn!(
                                        event_code = "subagent.sidecar.turn_auto.failed_after_all_busy_retry",
                                        error = %e,
                                        error_kind = ?kind,
                                        attempt = attempt + 1
                                    );
                                    if kind == TaskErrorKind::Auth {
                                        if let Err(kill_err) =
                                            self.pool.remove_worker_and_kill(&provider_id).await
                                        {
                                            error!(
                                                event_code = "subagent.pool.auth_kill_failed",
                                                provider_id = %provider_id,
                                                error = %kill_err,
                                                "remove_worker_and_kill failed after auth error"
                                            );
                                        }
                                    }
                                    return Err(sidecar_to_anyhow(e));
                                }
                            }
                        }
                        // Retries exhausted — surface HotSessionLimit so the
                        // manager can re-queue the task (or mark as failed if
                        // the deadline has passed).
                        warn!(
                            event_code = "subagent.session.hot_limit_all_busy_exhausted",
                            subagent_id = %input.subagent_id,
                            retries = ALL_BUSY_MAX_RETRIES,
                            "all hot sessions busy after retries — surfacing as HotSessionLimit"
                        );
                        Err(anyhow::Error::from(SubagentError::HotSessionLimit {
                            candidate: String::new(),
                        }))
                    }
                    HotLimitOutcome::ProtocolViolation(msg) => {
                        warn!(
                            event_code = "subagent.session.hot_limit_protocol_violation",
                            subagent_id = %input.subagent_id,
                            error = %msg,
                            "sidecar returned HOT_SESSION_LIMIT_REACHED without a valid candidate or all_busy flag"
                        );
                        Err(anyhow::Error::from(SubagentError::Validation(msg)))
                    }
                }
            }
            Err(e) => {
                let kind = classify_sidecar_error(&e);
                warn!(
                    event_code = "subagent.sidecar.turn_auto.failed",
                    error = %e,
                    error_kind = ?kind
                );
                // Auth-fail kill: hard-remove + kill the worker so the next
                // execute() re-reads credentials. The 5-min restart window
                // is bypassed because the credential is bad — restarting
                // with the same bad key won't help.
                if kind == TaskErrorKind::Auth {
                    info!(
                        event_code = "subagent.pool.auth_kill",
                        provider_id = %provider_id,
                        "auth failure detected — hard-killing + removing worker"
                    );
                    if let Err(kill_err) = self.pool.remove_worker_and_kill(&provider_id).await {
                        error!(
                            event_code = "subagent.pool.auth_kill_failed",
                            provider_id = %provider_id,
                            error = %kill_err,
                            "remove_worker_and_kill failed after auth error"
                        );
                    }
                }
                // Propagate the error. The caller (SubagentManager) records
                // `error_kind` via the ExecutorOutput — but since execute()
                // returns Err, the manager doesn't get an ExecutorOutput.
                // The classification is still useful for logging + future
                // retry logic (Phase 4). When the sidecar returns a "failed"
                // status (not an RPC error), parse_turn_auto_result handles
                // classification via the result payload.
                Err(sidecar_to_anyhow(e))
            }
        }
    }
}

/// Classify a `SidecarError` into a `TaskErrorKind` for recovery strategy
/// selection (Phase 3 Task 3).
///
/// Error code table (spec §4.2 + Phase 3 extensions):
/// - `-32010` (AUTH_FAILURE) → `TaskErrorKind::Auth` → hard kill + remove worker
/// - `-32011` (RATE_LIMIT) → `TaskErrorKind::RateLimit` → keep worker, backoff
/// - `-32012` (NETWORK_ERROR) → `TaskErrorKind::Network` → keep worker, retry
/// - `-32003` (TASK_TIMEOUT) → `TaskErrorKind::Timeout` → keep worker, retry
/// - `SidecarError::Crashed` → `TaskErrorKind::Crash` → crash-restart logic
/// - `SidecarError::Spawn` with "connection refused" → `TaskErrorKind::Network`
/// - everything else → `TaskErrorKind::Unknown`
pub fn classify_sidecar_error(err: &SidecarError) -> TaskErrorKind {
    match err {
        SidecarError::Application(code, _msg, _data) => {
            match *code {
                AUTH_FAILURE => TaskErrorKind::Auth,
                RATE_LIMIT => TaskErrorKind::RateLimit,
                NETWORK_ERROR => TaskErrorKind::Network,
                TASK_TIMEOUT => TaskErrorKind::Timeout,
                HOT_SESSION_LIMIT_REACHED => TaskErrorKind::HotSessionLimit,
                // Other application codes (SESSION_NOT_FOUND,
                // SIDECAR_UNHEALTHY, PROFILE_NOT_FOUND, TOOL_NOT_ALLOWED,
                // INVALID_OUTPUT_SCHEMA, PROTOCOL_MISMATCH) are handled by
                // their respective flows (eviction, profile resolution, etc.)
                // — classify as Unknown for the general error-handling path.
                _ => TaskErrorKind::Unknown,
            }
        }
        SidecarError::Crashed(_) => TaskErrorKind::Crash,
        SidecarError::Spawn(msg) => {
            // Spawn failures with "connection refused" indicate a network
            // issue (sidecar binary can't be reached / started). Other spawn
            // failures (missing binary, permissions) are Unknown.
            if msg.contains("connection refused") {
                TaskErrorKind::Network
            } else {
                TaskErrorKind::Unknown
            }
        }
        // Rpc, Timeout, Io, are less specific — classify by variant.
        SidecarError::Timeout(_) => TaskErrorKind::Timeout,
        SidecarError::Io(msg) => {
            // IO errors with "connection refused" or "broken pipe" are
            // network-related. Other IO errors are Unknown.
            if msg.contains("connection refused") || msg.contains("network") {
                TaskErrorKind::Network
            } else {
                TaskErrorKind::Unknown
            }
        }
        SidecarError::Rpc(_) => TaskErrorKind::Unknown,
    }
}

/// Outcome of classifying a `HOT_SESSION_LIMIT_REACHED` error's `data` field
/// (Bug 1 fix). Distinguishes three cases that the old `Option<String>`
/// return collapsed into one, causing the protocol-violation path to be
/// mislabeled as "all busy" and surface as `HotSessionLimit`.
#[derive(Debug)]
enum HotLimitOutcome {
    /// A specific LRU session was named as the eviction candidate.
    Evict(String),
    /// All hot sessions are in-use (busy) — no candidate is evictable. The
    /// sidecar signals this with `data.all_busy = true` +
    /// `data.candidate = null`.
    AllBusy,
    /// The sidecar response is malformed: `data` is missing, or
    /// `data.candidate` is missing/null without `data.all_busy = true`.
    /// This is a sidecar protocol violation, not a capacity condition.
    ProtocolViolation(String),
}

/// Classify the `data` field of a `HOT_SESSION_LIMIT_REACHED` JSON-RPC error
/// into one of three outcomes (Bug 1 fix).
///
/// The sidecar is the hot-pool authority (spec §4.4):
/// - `data.candidate = "<session_id>"` → evict that session.
/// - `data.candidate = null` + `data.all_busy = true` → all sessions are
///   in-use (running a turn); skip eviction, surface `HotSessionLimit`.
/// - `data` missing or `data.candidate` missing without `all_busy` → sidecar
///   protocol violation; surface a `Validation` error with a clear message.
fn classify_hot_limit_error(data: Option<&serde_json::Value>) -> HotLimitOutcome {
    let Some(d) = data else {
        return HotLimitOutcome::ProtocolViolation(
            "HOT_SESSION_LIMIT_REACHED error missing data field".into(),
        );
    };
    if d.get("all_busy").and_then(|v| v.as_bool()).unwrap_or(false) {
        return HotLimitOutcome::AllBusy;
    }
    match d
        .get("candidate")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
    {
        Some(c) => HotLimitOutcome::Evict(c.to_string()),
        None => HotLimitOutcome::ProtocolViolation(
            "HOT_SESSION_LIMIT_REACHED error missing data.candidate".into(),
        ),
    }
}

impl SidecarTaskExecutor {
    /// Proactively hibernate the LRU hot session (spec §8.3 step 1).
    /// Unlike `evict_session` (reactive, sidecar-named candidate), this picks
    /// the LRU from the DB and calls `evict_session` with its adapter_session_id.
    /// Used by `PressureResponder` (§8.3 step 1) when system memory pressure
    /// is detected — proactively sheds one hot session to free RSS.
    ///
    /// **C7 fix (pool-wide LRU):** iterates ALL supervisors in the pool via
    /// `for_each_supervisor` (clone Arcs under lock, do async eviction after).
    /// For each supervisor, queries `find_lru_hot_binding` with that
    /// supervisor's `harness_name`. Collects all candidates across all
    /// providers, picks the globally-oldest by `last_used_at_ms`, then calls
    /// `evict_session`. This preserves the I5 fix (pool-wide LRU) with the
    /// correct per-supervisor harness lookup.
    pub async fn evict_lru(&self) -> anyhow::Result<()> {
        // Step 1: collect harness names under the workers map lock ONLY.
        // No DB access under the map lock — avoids DB->workers lock ordering
        // (supervisor_for_session does workers->DB, so we must NOT nest
        // DB->workers here).
        let harnesses: Vec<String> = {
            let mut names: Vec<String> = Vec::new();
            self.pool.for_each_supervisor(|_pid, sup| {
                names.push(sup.config().harness_name.clone());
            });
            names
        };

        // Step 2: acquire DB lock + query LRU for each harness. The workers
        // map lock is NOT held here - correct lock ordering.
        let candidates_for_lru: Vec<(SubagentHarnessBindingRow, String)> = {
            let Some(db) = &self.db else {
                return Err(SubagentError::Validation(
                    "evict_lru requires a DB connection; no-DB executor cannot pick LRU".into(),
                )
                .into());
            };
            let db = db.lock().expect("db lock poisoned");
            let mut all_candidates: Vec<(SubagentHarnessBindingRow, String)> = Vec::new();
            for harness in &harnesses {
                if let Ok(Some(binding)) = db.subagent_find_lru_hot_binding(harness) {
                    all_candidates.push((binding, harness.clone()));
                }
            }
            // Drop db lock before any async work.
            all_candidates
        };

        // Step 2: pick the globally-oldest by last_used_at_ms (ascending
        // order — oldest first). `last_used_at_ms` is `Option<i64>`; treat
        // `None` as the oldest (epoch 0) so bindings that were never used
        // get evicted first.
        let oldest = candidates_for_lru
            .into_iter()
            .min_by_key(|(binding, _)| binding.last_used_at_ms.unwrap_or(0));

        let Some((binding, _harness)) = oldest else {
            info!(
                event_code = "subagent.pressure.no_lru",
                "no hot binding to hibernate"
            );
            return Ok(());
        };

        let adapter_session_id = binding.adapter_session_id.ok_or_else(|| {
            SubagentError::Validation("LRU hot binding has no adapter_session_id".into())
        })?;

        // Step 3: delegate to evict_session (resolves the supervisor via
        // supervisor_for_session — pool-wide routing).
        self.evict_session(&adapter_session_id).await?;
        Ok(())
    }

    /// Drive the eviction flow for a single session (spec §4.4):
    /// 1. RPC: `session.prepare_hibernate(adapter_session_id)` → `{memory_delta, stats}`
    /// 2. Persist: write memory delta (optional) + flip binding atomically
    ///    (`commit_eviction`, which computes the logical `warm`/`cold` status
    ///    from the final memory state per §3.3) + write `session_hibernate`
    ///    resource event. The DB lock (`std::sync::Mutex`) is acquired in a
    ///    scoped block and released before the `.await` on `session.close` —
    ///    never held across an RPC call.
    /// 3. RPC: `session.close(adapter_session_id)` — failure is FATAL (see
    ///    `SidecarTaskExecutor` doc comment).
    ///
    /// **C7 fix (pool routing):** the supervisor is resolved via
    /// `self.pool.supervisor_for_session(adapter_session_id)`. If not found
    /// (binding belongs to a removed provider, or the sidecar/DB are out of
    /// sync) → log warning + return a fatal error. Silently skipping would
    /// cause the `turn_auto` retry to hit `HOT_SESSION_LIMIT_REACHED` again,
    /// hiding the root cause behind the symptom.
    ///
    /// **Bug 2 fix:** returns `Result<(), SubagentError>` (not
    /// `anyhow::Result<()>`) so structured domain errors survive the
    /// `anyhow::Error` round-trip through `execute()`. The caller
    /// (`execute_task`) downcasts `anyhow::Error → SubagentError` — bare
    /// `anyhow::anyhow!(...)` errors fail the downcast and degrade to
    /// `SubagentError::Store` ("database error"), losing the root cause.
    /// Only real SQLite/store failures use `SubagentError::Store`; all
    /// other eviction failures use semantically-correct variants
    /// (`HotSessionStateDivergence` for sidecar/DB sync issues,
    /// `SidecarRpc` for close failures, `Validation` for config errors).
    async fn evict_session(
        &self,
        adapter_session_id: &str,
    ) -> std::result::Result<EvictionOutcome, SubagentError> {
        // No-DB path: eviction cannot be performed safely. The persist step
        // below (write memory + flip binding atomically) is what keeps the DB
        // and sidecar in sync; without a DB we would skip it, call `close`,
        // and the sidecar would release its slot while the DB still believes
        // the session is hot — silent state divergence. Surface this as a
        // fatal error BEFORE any RPC so the caller knows a sidecar restart is
        // the recovery path. (Mirrors the `with_pool` doc comment contract.)
        if self.db.is_none() {
            error!(
                event_code = "subagent.session.eviction_no_db",
                adapter_session_id = %adapter_session_id,
                "eviction requested but executor has no DB handle — cannot persist binding flip atomically; aborting to avoid state divergence"
            );
            return Err(SubagentError::Validation(format!(
                "eviction requires a DB connection for atomic persistence; \
                 no-DB executor cannot evict safely (adapter_session_id={adapter_session_id})"
            )));
        }

        // C7 fix: resolve which supervisor owns this session via the pool.
        let (_provider_id, supervisor) = match self.pool.supervisor_for_session(adapter_session_id)
        {
            Ok(Some(pair)) => pair,
            Ok(None) => {
                // With the two-phase session lifecycle (pending → active), a
                // candidate named by the sidecar should ALWAYS have a DB
                // binding: only activated sessions are in the LRU, and
                // activation only happens AFTER the DB binding is committed
                // (manager calls `session.activate` post-commit). A candidate
                // with no binding is therefore a permanent sidecar/DB state
                // divergence — NOT a transient timing window.
                //
                // Previously (pre-two-phase), `endTurn()` made a session
                // evictable BEFORE Rust committed the binding, creating a
                // timing window where `purge_session` could close a session
                // whose binding was about to be committed. The two-phase
                // lifecycle eliminates that window: pending sessions are never
                // in the LRU and thus never selected as eviction candidates.
                //
                // Surfacing as `HotSessionStateDivergence` (fatal) instead of
                // `HotSessionLimit` (transient) ensures the task fails loudly
                // rather than silently retrying into a broken pool state.
                error!(
                    event_code = "subagent.session.eviction_state_divergence",
                    adapter_session_id = %adapter_session_id,
                    "eviction candidate has no DB binding — sidecar/DB state divergence \
                     (with two-phase lifecycle, all LRU candidates must have committed bindings)"
                );
                return Err(SubagentError::HotSessionStateDivergence(
                    format!(
                        "no binding found in DB for adapter_session_id={adapter_session_id}; \
                         sidecar/DB state divergence (sidecar returned LRU candidate with no binding; \
                         with two-phase lifecycle this should be impossible — pending sessions \
                         are never in LRU)"
                    ),
                ));
            }
            Err(e) => {
                // DB query error — transient, not permanent divergence.
                // The binding might exist but the query failed (disk I/O,
                // lock contention, etc.). Return `HotSessionLimit` so the
                // caller re-queues the task for a later retry, rather than
                // fatally failing on a transient DB error.
                warn!(
                    event_code = "subagent.session.supervisor_lookup_db_error",
                    adapter_session_id = %adapter_session_id,
                    error = %e,
                    "DB query failed in supervisor_for_session — treating as transient"
                );
                return Err(SubagentError::HotSessionLimit {
                    candidate: adapter_session_id.to_string(),
                });
            }
        };

        let handle = supervisor
            .ensure_started()
            .await
            .map_err(SubagentError::from)?;
        // 1. prepare_hibernate → memory delta
        // Bug 1 concurrent eviction fix: if the session was already closed
        // by a concurrent evictor, prepare_hibernate returns SESSION_NOT_FOUND
        // (-32001). This is NOT an error — the eviction was already done.
        // Return `AlreadyEvicted` so the caller retries `turn_auto`.
        let hibernate_result = match handle.prepare_hibernate(adapter_session_id).await {
            Ok(r) => r,
            Err(SidecarError::Application(code, _msg, _data)) if code == SESSION_NOT_FOUND => {
                info!(
                    event_code = "subagent.session.eviction_already_evicted",
                    adapter_session_id = %adapter_session_id,
                    "prepare_hibernate returned SESSION_NOT_FOUND — concurrent evictor already closed this session"
                );
                return Ok(EvictionOutcome::AlreadyEvicted);
            }
            Err(SidecarError::Application(code, _msg, _data))
                if code == HOT_SESSION_LIMIT_REACHED =>
            {
                // The sidecar atomically rejected this eviction because the
                // session became busy after candidate selection. Treat it as
                // transient capacity contention; the caller can retry after
                // the active turn releases the session.
                warn!(
                    event_code = "subagent.session.eviction_busy",
                    adapter_session_id = %adapter_session_id,
                    "eviction candidate became busy before prepare_hibernate"
                );
                return Err(SubagentError::HotSessionLimit {
                    candidate: adapter_session_id.to_string(),
                });
            }
            Err(e) => {
                warn!(
                    event_code = "subagent.session.eviction_prepare_failed",
                    error = %e
                );
                return Err(SubagentError::from(e));
            }
        };
        let memory_delta = hibernate_result.get("memory_delta").cloned();
        let stats = hibernate_result.get("stats").cloned();

        // 2. Persist: write memory + flip binding (atomic) + event.
        //    All DB writes happen in this scoped block; the lock is released
        //    before the `session.close` `.await` below.
        if let Some(db) = &self.db {
            let harness = supervisor.config().harness_name.clone();
            let (subagent_id, hot_summary_written) = {
                let db_guard = db.lock().expect("db lock poisoned");
                // Bug 1 concurrent eviction fix: first try to find a HOT
                // binding. If none exists, check if a non-hot binding exists
                // (is_hot=0) — that means a concurrent evictor already
                // flipped it. In that case, return `AlreadyEvicted` (not an
                // error). Only if NO binding exists at all is it a real
                // state divergence.
                let binding = db_guard
                    .subagent_find_hot_binding_by_session(adapter_session_id, &harness)
                    .map_err(|e| {
                        SubagentError::Store(anyhow::anyhow!("find hot binding failed: {e}"))
                    })?;
                let binding = match binding {
                    Some(b) => b,
                    None => {
                        // No hot binding — check if any binding exists.
                        let any_binding = db_guard
                            .subagent_find_binding_by_session(adapter_session_id, &harness)
                            .map_err(|e| {
                                SubagentError::Store(anyhow::anyhow!(
                                    "find binding (any) failed: {e}"
                                ))
                            })?;
                        match any_binding {
                            Some(b) if b.is_hot == 0 => {
                                // Concurrent evictor already flipped the binding.
                                info!(
                                    event_code = "subagent.session.eviction_already_evicted",
                                    adapter_session_id = %adapter_session_id,
                                    "hot binding already flipped (is_hot=0) — concurrent evictor completed eviction"
                                );
                                return Ok(EvictionOutcome::AlreadyEvicted);
                            }
                            _ => {
                                // No binding at all — real state divergence.
                                return Err(SubagentError::HotSessionStateDivergence(
                                    format!(
                                        "no binding found in DB for adapter_session_id={adapter_session_id}; \
                                         sidecar/DB state divergence (sidecar holds session, DB has no binding)"
                                    ),
                                ));
                            }
                        }
                    }
                };
                let subagent_id = binding.subagent_id.clone();
                // Compute the hot_summary to persist: only when memory_delta
                // is present, non-null, and has a `hot_summary` field. A null
                // or absent delta means prepare_hibernate produced no memory;
                // the subagent's final warm/cold status is then decided by
                // `commit_eviction` based on whether any prior hot_summary
                // exists (spec §3.3).
                let hot_summary: Option<&str> = memory_delta
                    .as_ref()
                    .filter(|d| !d.is_null())
                    .and_then(|d| d.get("hot_summary"))
                    .and_then(|v| v.as_str());
                let wrote_summary = hot_summary.is_some();
                // Atomic: optional hot_summary write + flip binding
                // (is_hot=0, status='closed', closed_at_ms=Some(now)) +
                // logical status computed from final memory state (§3.3:
                // 'warm' iff hot_summary IS NOT NULL, else 'cold').
                let now = busytok_domain::now_ms();
                let mut flipped = binding.clone();
                flipped.is_hot = 0;
                flipped.status = "closed".into();
                flipped.closed_at_ms = Some(now);
                let new_status = db_guard
                    .subagent_commit_eviction(&flipped, &subagent_id, hot_summary)
                    .map_err(|e| {
                        SubagentError::Store(anyhow::anyhow!("commit eviction failed: {e}"))
                    })?;
                // Explicit status transition log (§3.3): hot binding closed,
                // logical status flips to warm (hot_summary present) or cold.
                // `new_status` is the authoritative value computed by
                // `commit_eviction` from the post-write memory row state.
                info!(
                    event_code = "subagent.status.hot_to_warm",
                    subagent_id = %subagent_id,
                    adapter_session_id = %adapter_session_id,
                    new_status = %new_status,
                    "hot session evicted — logical status transitioned"
                );
                // Write `session_hibernate` resource event for observability.
                // Best-effort: this is pure observability. If it fails we must
                // NOT propagate — doing so would skip `session.close`, leaving
                // the DB flipped to closed while the sidecar still holds
                // the session hot (the exact state divergence the fatal-close
                // rule exists to prevent). Log and continue to `close`.
                if let Err(e) = db_guard.subagent_insert_resource_event(&SubagentResourceEventRow {
                    id: format!("re_{}", uuid::Uuid::new_v4()),
                    event_type: "session_hibernate".into(),
                    target_id: Some(subagent_id.clone()),
                    rss_mb: None,
                    cpu_percent: None,
                    detail_json: Some(
                        serde_json::to_string(&serde_json::json!({
                            "adapter_session_id": adapter_session_id,
                            "reason": "evicted",
                            "stats": stats,
                        }))
                        .unwrap_or_default(),
                    ),
                    created_at_ms: now,
                }) {
                    warn!(
                        event_code = "subagent.session.eviction_event_failed",
                        subagent_id = %subagent_id,
                        adapter_session_id = %adapter_session_id,
                        error = %e,
                        "insert resource event failed during eviction — continuing to close"
                    );
                }
                (subagent_id, wrote_summary)
            }; // db_guard dropped here — before the `.await` on close.

            info!(
                event_code = "subagent.session.evicted",
                subagent_id = %subagent_id,
                adapter_session_id = %adapter_session_id,
                wrote_hot_summary = hot_summary_written,
                "evicted LRU session"
            );
        }

        // 3. close — failure is FATAL. If the sidecar didn't release the
        //    slot, retrying turn_auto would hit HOT_SESSION_LIMIT_REACHED
        //    again and the sidecar/DB would diverge (DB says closed/warm,
        //    sidecar still holds the session hot). Propagate the error so
        //    the caller knows the pool is in an inconsistent state; a
        //    sidecar restart is the recovery path.
        if let Err(e) = handle.close(adapter_session_id).await {
            error!(
                event_code = "subagent.session.eviction_close_failed",
                adapter_session_id = %adapter_session_id,
                error = %e,
                "session.close failed during eviction — DB flipped but sidecar slot not released; \
                 aborting retry to avoid state divergence (sidecar restart may be needed)"
            );
            return Err(SubagentError::SidecarRpc {
                message: format!(
                    "session.close failed during eviction for {adapter_session_id}: {e} \
                     — sidecar pool may be inconsistent, restart recommended"
                ),
                code: None,
            });
        }
        Ok(EvictionOutcome::Evicted)
    }
}

/// Parse a `turn_auto` result payload into an `ExecutorOutput`.
///
/// Field semantics (spec §4.4 turn_auto response):
/// - `adapter_session_id`: backing session id (None/empty means warm path)
/// - `session_reused`: true if an existing hot session was reused
/// - `status`: "completed" | "failed" | "timeout"
/// - `result.task_summary`: short human-readable summary
/// - `result.memory_update`: structured memory delta (spec §4.3)
/// - `usage`: token/cost breakdown
fn parse_turn_auto_result(result: &serde_json::Value) -> ExecutorOutput {
    let adapter_session_id = result
        .get("adapter_session_id")
        .and_then(|v| v.as_str())
        .map(String::from);
    let session_reused = result
        .get("session_reused")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let status_str = result
        .get("status")
        .and_then(|v| v.as_str())
        .unwrap_or("completed");
    let status = match status_str {
        "completed" => TaskStatus::Completed,
        "failed" => TaskStatus::Failed,
        "timeout" => TaskStatus::Failed,
        _ => TaskStatus::Completed,
    };
    let summary = result
        .pointer("/result/task_summary")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let usage = result
        .get("usage")
        .map(|u| TaskUsage {
            model: u.get("model").and_then(|v| v.as_str()).map(String::from),
            provider: u.get("provider").and_then(|v| v.as_str()).map(String::from),
            input_tokens: u.get("input_tokens").and_then(|v| v.as_i64()),
            output_tokens: u.get("output_tokens").and_then(|v| v.as_i64()),
            cache_read_tokens: u.get("cache_read_tokens").and_then(|v| v.as_i64()),
            cache_write_tokens: u.get("cache_write_tokens").and_then(|v| v.as_i64()),
            cost_usd: u.get("cost_usd").and_then(|v| v.as_f64()),
        })
        .unwrap_or_default();

    // Extract memory_update (spec §4.3 result.memory_update). If absent,
    // MemoryUpdate::default() → current_state_summary None → MemoryUpdater
    // preserves the existing hot_summary (no overwrite).
    let memory_update = result
        .pointer("/result/memory_update")
        .map(parse_memory_update)
        .unwrap_or_default();

    ExecutorOutput {
        adapter_session_id,
        session_reused,
        status,
        summary,
        usage,
        memory_update,
        // Phase 3: error classification is added by Task 3's error handler
        // post-execution; the sidecar executor itself does not classify.
        error_kind: None,
    }
}

/// Test-visible wrapper around `parse_turn_auto_result`. The inner function
/// stays private to keep the call sites within this module; tests exercise the
/// parsing logic via this thin shim so they don't need to spin up a sidecar.
pub fn parse_turn_auto_result_for_test(result: &serde_json::Value) -> ExecutorOutput {
    parse_turn_auto_result(result)
}

/// Parse the `result.memory_update` object (spec §4.3) into a `MemoryUpdate`.
///
/// Each field is extracted defensively: missing/null fields fall back to
/// empty/None so a partial update from the sidecar still parses. A missing
/// `path` (KeyFile) or `question` (OpenQuestion) drops that entry via
/// `filter_map` — those are the identity keys, and an entry without them is
/// meaningless.
fn parse_memory_update(mu: &serde_json::Value) -> MemoryUpdate {
    use crate::memory::{KeyFile, OpenQuestion};
    let current_state_summary = mu
        .get("current_state_summary")
        .and_then(|v| v.as_str())
        .map(String::from);
    let key_files = mu
        .get("key_files")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|f| {
                    Some(KeyFile {
                        path: f.get("path")?.as_str()?.to_string(),
                        reason: f
                            .get("reason")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string(),
                        last_seen_at_ms: f
                            .get("last_seen_at_ms")
                            .and_then(|v| v.as_i64())
                            .unwrap_or(0),
                        score: f.get("score").and_then(|v| v.as_i64()).unwrap_or(0),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let decisions = mu
        .get("decisions")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    let open_questions = mu
        .get("open_questions")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|q| {
                    Some(OpenQuestion {
                        question: q.get("question")?.as_str()?.to_string(),
                        status: q
                            .get("status")
                            .and_then(|v| v.as_str())
                            .unwrap_or("open")
                            .to_string(),
                        created_at_ms: q.get("created_at_ms").and_then(|v| v.as_i64()).unwrap_or(0),
                        last_seen_at_ms: q
                            .get("last_seen_at_ms")
                            .and_then(|v| v.as_i64())
                            .unwrap_or(0),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    MemoryUpdate {
        current_state_summary,
        key_files,
        decisions,
        open_questions,
    }
}

/// Convert `SidecarError` → `SubagentError` (preserving application error codes)
/// → `anyhow::Error`. The `delegate()` method downcasts back to `SubagentError`
/// so the control contract (`subagent.profile_not_found`, etc.) is honored.
fn sidecar_to_anyhow(e: SidecarError) -> anyhow::Error {
    anyhow::Error::from(SubagentError::from(e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sidecar::protocol::*;

    // --- classify_sidecar_error tests (Step 1) ---

    #[test]
    fn classify_auth_failure() {
        // -32010 (AUTH_FAILURE) with "401 Unauthorized" message → Auth.
        let err = SidecarError::Application(AUTH_FAILURE, "401 Unauthorized".to_string(), None);
        assert_eq!(classify_sidecar_error(&err), TaskErrorKind::Auth);
    }

    #[test]
    fn classify_rate_limit() {
        // -32011 (RATE_LIMIT) with "429 Too Many Requests" → RateLimit.
        let err = SidecarError::Application(RATE_LIMIT, "429 Too Many Requests".to_string(), None);
        assert_eq!(classify_sidecar_error(&err), TaskErrorKind::RateLimit);
    }

    #[test]
    fn classify_network() {
        // -32012 (NETWORK_ERROR) with "connection refused" → Network.
        let err = SidecarError::Application(NETWORK_ERROR, "connection refused".to_string(), None);
        assert_eq!(classify_sidecar_error(&err), TaskErrorKind::Network);
    }

    #[test]
    fn classify_timeout() {
        // -32003 (TASK_TIMEOUT) → Timeout.
        let err = SidecarError::Application(TASK_TIMEOUT, "task timed out".to_string(), None);
        assert_eq!(classify_sidecar_error(&err), TaskErrorKind::Timeout);
    }

    #[test]
    fn classify_hot_session_limit() {
        // -32002 (HOT_SESSION_LIMIT_REACHED) → HotSessionLimit (NOT Unknown).
        // This ensures the failed_after_eviction and failed_after_all_busy_retry
        // log paths emit error_kind=HotSessionLimit instead of Unknown when
        // a HOT_SESSION_LIMIT_REACHED error reaches the generic error handler.
        let err = SidecarError::Application(
            HOT_SESSION_LIMIT_REACHED,
            "hot session limit reached".to_string(),
            None,
        );
        assert_eq!(classify_sidecar_error(&err), TaskErrorKind::HotSessionLimit);
    }

    #[test]
    fn classify_crash() {
        // SidecarError::Crashed → Crash.
        let err = SidecarError::Crashed("sidecar exited with code 1".to_string());
        assert_eq!(classify_sidecar_error(&err), TaskErrorKind::Crash);
    }

    #[test]
    fn classify_spawn_network() {
        // SidecarError::Spawn with "connection refused" → Network.
        let err = SidecarError::Spawn("connection refused".to_string());
        assert_eq!(classify_sidecar_error(&err), TaskErrorKind::Network);
    }

    #[test]
    fn classify_unknown() {
        // Everything else → Unknown.
        // An unmapped application code:
        let err = SidecarError::Application(-32099, "some unknown error".to_string(), None);
        assert_eq!(classify_sidecar_error(&err), TaskErrorKind::Unknown);

        // A non-network spawn error:
        let err = SidecarError::Spawn("permission denied".to_string());
        assert_eq!(classify_sidecar_error(&err), TaskErrorKind::Unknown);

        // An RPC error:
        let err = SidecarError::Rpc("internal error".to_string());
        assert_eq!(classify_sidecar_error(&err), TaskErrorKind::Unknown);
    }

    // --- error code regression test ---

    #[test]
    fn new_error_codes_do_not_overlap_existing_protocol_constants() {
        // Existing protocol constants: -32001..-32008.
        let existing: &[i32] = &[
            SESSION_NOT_FOUND,         // -32001
            HOT_SESSION_LIMIT_REACHED, // -32002
            TASK_TIMEOUT,              // -32003
            SIDECAR_UNHEALTHY,         // -32004
            PROFILE_NOT_FOUND,         // -32005
            TOOL_NOT_ALLOWED,          // -32006
            INVALID_OUTPUT_SCHEMA,     // -32007
            PROTOCOL_MISMATCH,         // -32008
        ];
        // New Phase 3 codes: -32010..-32012.
        let new_codes: &[i32] = &[AUTH_FAILURE, RATE_LIMIT, NETWORK_ERROR];

        // Verify existing codes are in the expected range.
        for &code in existing {
            assert!(
                (-32008..=-32001).contains(&code),
                "existing code {code} outside expected range -32001..-32008"
            );
        }
        // Verify new codes are in the expected range.
        for &code in new_codes {
            assert!(
                (-32012..=-32010).contains(&code),
                "new code {code} outside expected range -32010..-32012"
            );
        }
        // Verify NO overlap between existing and new codes.
        for &new_code in new_codes {
            assert!(
                !existing.contains(&new_code),
                "new code {new_code} overlaps existing protocol constants"
            );
        }
        // Verify the gap (-32009) is unused (reserved for future use).
        assert!(
            !existing.contains(&-32009) && !new_codes.contains(&-32009),
            "code -32009 should be unused (gap between existing and new ranges)"
        );
    }

    // --- classify_sidecar_error variant coverage ---

    #[test]
    fn classify_timeout_variant() {
        // SidecarError::Timeout (the variant, not the application code) → Timeout.
        let err = SidecarError::Timeout("rpc timed out".to_string());
        assert_eq!(classify_sidecar_error(&err), TaskErrorKind::Timeout);
    }

    #[test]
    fn classify_io_network() {
        // SidecarError::Io with "connection refused" → Network.
        let err = SidecarError::Io("connection refused".to_string());
        assert_eq!(classify_sidecar_error(&err), TaskErrorKind::Network);
    }

    #[test]
    fn classify_io_unknown() {
        // SidecarError::Io without network keywords → Unknown.
        let err = SidecarError::Io("disk full".to_string());
        assert_eq!(classify_sidecar_error(&err), TaskErrorKind::Unknown);
    }

    // --- classify_hot_limit_error tests (Bug 1 fix) ---

    #[test]
    fn hot_limit_classify_evict_candidate() {
        // Normal case: candidate is a non-empty session id → Evict.
        let data = serde_json::json!({"candidate": "pi_sess_1"});
        match classify_hot_limit_error(Some(&data)) {
            HotLimitOutcome::Evict(c) => assert_eq!(c, "pi_sess_1"),
            other => panic!("expected Evict, got {other:?}"),
        }
    }

    #[test]
    fn hot_limit_classify_all_busy() {
        // All sessions in-use: candidate=null + all_busy=true → AllBusy.
        let data = serde_json::json!({"candidate": null, "all_busy": true});
        match classify_hot_limit_error(Some(&data)) {
            HotLimitOutcome::AllBusy => {}
            other => panic!("expected AllBusy, got {other:?}"),
        }
    }

    #[test]
    fn hot_limit_classify_all_busy_takes_priority() {
        // If all_busy=true, return AllBusy even if a candidate is present —
        // the candidate might be a busy session that must not be evicted.
        let data = serde_json::json!({"candidate": "pi_sess_1", "all_busy": true});
        match classify_hot_limit_error(Some(&data)) {
            HotLimitOutcome::AllBusy => {}
            other => panic!("expected AllBusy, got {other:?}"),
        }
    }

    #[test]
    fn hot_limit_classify_all_busy_false_falls_through_to_candidate() {
        // all_busy present but false → check candidate normally.
        let data = serde_json::json!({"candidate": "pi_sess_1", "all_busy": false});
        match classify_hot_limit_error(Some(&data)) {
            HotLimitOutcome::Evict(c) => assert_eq!(c, "pi_sess_1"),
            other => panic!("expected Evict, got {other:?}"),
        }
    }

    #[test]
    fn hot_limit_classify_protocol_violation_no_data() {
        // data field missing entirely → ProtocolViolation.
        match classify_hot_limit_error(None) {
            HotLimitOutcome::ProtocolViolation(msg) => {
                assert!(msg.contains("missing data"), "got: {msg}");
            }
            other => panic!("expected ProtocolViolation, got {other:?}"),
        }
    }

    #[test]
    fn hot_limit_classify_protocol_violation_no_candidate() {
        // data present but candidate missing, all_busy not set → ProtocolViolation.
        let data = serde_json::json!({"foo": "bar"});
        match classify_hot_limit_error(Some(&data)) {
            HotLimitOutcome::ProtocolViolation(msg) => {
                assert!(msg.contains("missing data.candidate"), "got: {msg}");
            }
            other => panic!("expected ProtocolViolation, got {other:?}"),
        }
    }

    #[test]
    fn hot_limit_classify_protocol_violation_empty_candidate() {
        // candidate is an empty string → ProtocolViolation (treated as missing).
        let data = serde_json::json!({"candidate": ""});
        match classify_hot_limit_error(Some(&data)) {
            HotLimitOutcome::ProtocolViolation(msg) => {
                assert!(msg.contains("missing data.candidate"), "got: {msg}");
            }
            other => panic!("expected ProtocolViolation, got {other:?}"),
        }
    }
}
