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
    AUTH_FAILURE, HOT_SESSION_LIMIT_REACHED, NETWORK_ERROR, RATE_LIMIT, TASK_TIMEOUT,
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
}

#[async_trait]
impl TaskExecutor for SidecarTaskExecutor {
    async fn execute(&self, input: &ExecutorInput) -> anyhow::Result<ExecutorOutput> {
        // Step 1: extract provider_id. None → error (cannot route).
        let provider_id = input.provider_id.as_ref().ok_or_else(|| {
            anyhow::anyhow!("profile not bound to a provider — cannot route execute()")
        })?;

        // Step 2: ensure_worker (synchronous — I2 fix).
        let supervisor = self
            .pool
            .ensure_worker(provider_id)
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
                info!(
                    event_code = "subagent.session.hot_limit_reached",
                    subagent_id = %input.subagent_id,
                    "hot session limit reached, driving eviction"
                );
                let candidate = extract_candidate_from_data(data.as_ref())?;
                self.evict_session(&candidate).await?;
                // Retry turn_auto after eviction. The sidecar's session pool
                // now has a free slot (close released it), so this should
                // succeed; any failure propagates as a normal turn_auto error.
                match handle.turn_auto(params).await {
                    Ok(result) => Ok(parse_turn_auto_result(&result)),
                    Err(e) => {
                        let kind = classify_sidecar_error(&e);
                        warn!(
                            event_code = "subagent.sidecar.turn_auto.failed_after_eviction",
                            error = %e,
                            error_kind = ?kind
                        );
                        // Auth-fail kill: hard-remove + kill the worker so the
                        // next execute() re-reads credentials (the bad key
                        // might have been refreshed in the keychain).
                        if kind == TaskErrorKind::Auth {
                            if let Err(kill_err) =
                                self.pool.remove_worker_and_kill(provider_id).await
                            {
                                error!(
                                    event_code = "subagent.pool.auth_kill_failed",
                                    provider_id = %provider_id,
                                    error = %kill_err,
                                    "remove_worker_and_kill failed after auth error"
                                );
                            }
                        }
                        Err(sidecar_to_anyhow(e))
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
                    if let Err(kill_err) = self.pool.remove_worker_and_kill(provider_id).await {
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
                // Other application codes (SESSION_NOT_FOUND,
                // HOT_SESSION_LIMIT_REACHED, SIDECAR_UNHEALTHY,
                // PROFILE_NOT_FOUND, TOOL_NOT_ALLOWED, INVALID_OUTPUT_SCHEMA,
                // PROTOCOL_MISMATCH) are handled by their respective flows
                // (eviction, profile resolution, etc.) — classify as Unknown
                // for the general error-handling path.
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

/// Extract the LRU candidate `adapter_session_id` from the JSON-RPC error's
/// `data.candidate` field. The sidecar is the hot-pool authority (spec §4.4) —
/// it names the LRU session in the error response, so we read it directly
/// rather than querying the local DB. A missing/malformed `candidate` is a
/// sidecar protocol violation.
fn extract_candidate_from_data(data: Option<&serde_json::Value>) -> anyhow::Result<String> {
    let candidate = data
        .and_then(|d| d.get("candidate"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "HOT_SESSION_LIMIT_REACHED error missing data.candidate — \
                 sidecar protocol violation"
            )
        })?;
    Ok(candidate.to_string())
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
        // Step 1: collect (supervisor, harness_name) pairs under the map
        // lock. The closure must be trivial (clone+collect) — no async work
        // under the lock (Task 2 review note).
        let candidates_for_lru: Vec<(SubagentHarnessBindingRow, String)> = {
            let Some(db) = &self.db else {
                return Err(anyhow::anyhow!(
                    "evict_lru requires a DB connection; no-DB executor cannot pick LRU"
                ));
            };
            let db = db.lock().expect("db lock poisoned");
            // Collect harness names under the map lock (clone only — no async
            // work under the lock, per Task 2 review note).
            let mut harnesses: Vec<String> = Vec::new();
            self.pool.for_each_supervisor(|_pid, sup| {
                harnesses.push(sup.config().harness_name.clone());
            });
            // Query LRU for each harness (DB lock still held — sync query).
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

        let adapter_session_id = binding
            .adapter_session_id
            .ok_or_else(|| anyhow::anyhow!("LRU hot binding has no adapter_session_id"))?;

        // Step 3: delegate to evict_session (resolves the supervisor via
        // supervisor_for_session — pool-wide routing).
        self.evict_session(&adapter_session_id).await
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
    async fn evict_session(&self, adapter_session_id: &str) -> anyhow::Result<()> {
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
            return Err(anyhow::anyhow!(
                "eviction requires a DB connection for atomic persistence; \
                 no-DB executor cannot evict safely (adapter_session_id={adapter_session_id})"
            ));
        }

        // C7 fix: resolve which supervisor owns this session via the pool.
        let (_provider_id, supervisor) = match self.pool.supervisor_for_session(adapter_session_id)
        {
            Some(pair) => pair,
            None => {
                // No supervisor owns this session: either the binding belongs
                // to a removed provider (session already gone), or the sidecar
                // and DB are out of sync (the sidecar named a candidate the DB
                // doesn't track). Surface this as a fatal error — silently
                // skipping would cause the turn_auto retry to hit
                // HOT_SESSION_LIMIT_REACHED again, hiding the root cause
                // (out-of-sync) behind the symptom (limit reached).
                warn!(
                    event_code = "subagent.session.eviction_no_supervisor",
                    adapter_session_id = %adapter_session_id,
                    "no supervisor owns this session — binding may belong to a removed provider, or sidecar/DB are out of sync; aborting eviction"
                );
                return Err(anyhow::anyhow!(
                    "no hot binding found for adapter_session_id {adapter_session_id} \
                     — no supervisor owns this session (binding may belong to a removed \
                     provider, or sidecar/DB are out of sync)"
                ));
            }
        };

        let handle = supervisor
            .ensure_started()
            .await
            .map_err(sidecar_to_anyhow)?;
        // 1. prepare_hibernate → memory delta
        let hibernate_result = handle
            .prepare_hibernate(adapter_session_id)
            .await
            .map_err(|e| {
                warn!(
                    event_code = "subagent.session.eviction_prepare_failed",
                    error = %e
                );
                sidecar_to_anyhow(e)
            })?;
        let memory_delta = hibernate_result.get("memory_delta").cloned();
        let stats = hibernate_result.get("stats").cloned();

        // 2. Persist: write memory + flip binding (atomic) + event.
        //    All DB writes happen in this scoped block; the lock is released
        //    before the `session.close` `.await` below.
        if let Some(db) = &self.db {
            let harness = supervisor.config().harness_name.clone();
            let (subagent_id, hot_summary_written) = {
                let db_guard = db.lock().expect("db lock poisoned");
                // Find the binding for this adapter_session_id.
                let binding = db_guard
                    .subagent_find_hot_binding_by_session(adapter_session_id, &harness)
                    .map_err(|e| anyhow::anyhow!("find binding failed: {e}"))?
                    .ok_or_else(|| {
                        anyhow::anyhow!(
                            "no hot binding found for adapter_session_id {adapter_session_id}"
                        )
                    })?;
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
                    .map_err(|e| anyhow::anyhow!("commit eviction failed: {e}"))?;
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
            return Err(anyhow::anyhow!(
                "session.close failed during eviction for {adapter_session_id}: {e} \
                 — sidecar pool may be inconsistent, restart recommended"
            ));
        }
        Ok(())
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
}
