use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use tracing::{error, info, warn};

use busytok_store::{Database, SubagentResourceEventRow};

use crate::error::SubagentError;
use crate::mock_executor::{ExecutorInput, ExecutorOutput, TaskExecutor};
use crate::models::{TaskStatus, TaskUsage};
use crate::sidecar::supervisor::PiSidecarSupervisor;
use crate::sidecar::{protocol::HOT_SESSION_LIMIT_REACHED, SidecarError};

/// Drives a single subagent turn through the Pi sidecar.
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
    supervisor: Arc<PiSidecarSupervisor>,
    db: Option<Arc<Mutex<Database>>>,
}

impl SidecarTaskExecutor {
    pub fn new(supervisor: Arc<PiSidecarSupervisor>) -> Self {
        Self {
            supervisor,
            db: None,
        }
    }

    /// Construct with a DB handle for the eviction flow (production path).
    /// Without a DB the executor cannot persist memory deltas or flip
    /// bindings, so `HOT_SESSION_LIMIT_REACHED` surfaces as a fatal error.
    pub fn with_db(supervisor: Arc<PiSidecarSupervisor>, db: Arc<Mutex<Database>>) -> Self {
        Self {
            supervisor,
            db: Some(db),
        }
    }
}

#[async_trait]
impl TaskExecutor for SidecarTaskExecutor {
    async fn execute(&self, input: &ExecutorInput) -> anyhow::Result<ExecutorOutput> {
        let handle = self
            .supervisor
            .ensure_started()
            .await
            .map_err(sidecar_to_anyhow)?;
        // Note: `tools`, `prompt_artifact_ref`, and `memory_snapshot` are
        // deferred to Plan 4 (ContextBuilder). Plan 2 sends the minimal set.
        let params = serde_json::json!({
            "logical_subagent_id": input.subagent_id,
            "logical_subagent_name": input.subagent_name,
            "cwd": input.cwd,
            "profile": input.profile,
            "model": input.model,
            "prompt": input.prompt,
            "timeout_ms": input.timeout_seconds.map(|s| s * 1000),
        });
        info!(
            event_code = "subagent.sidecar.turn_auto.start",
            subagent_id = %input.subagent_id,
            profile = %input.profile,
            "sending turn_auto to sidecar"
        );
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
                let result = handle.turn_auto(params).await.map_err(|e| {
                    warn!(
                        event_code = "subagent.sidecar.turn_auto.failed_after_eviction",
                        error = %e
                    );
                    sidecar_to_anyhow(e)
                })?;
                Ok(parse_turn_auto_result(&result))
            }
            Err(e) => {
                warn!(
                    event_code = "subagent.sidecar.turn_auto.failed",
                    error = %e
                );
                Err(sidecar_to_anyhow(e))
            }
        }
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
    /// Drive the eviction flow for a single session (spec §4.4):
    /// 1. RPC: `session.prepare_hibernate(adapter_session_id)` → `{memory_delta, stats}`
    /// 2. Persist: write memory delta + flip binding atomically
    ///    (`commit_hibernate_binding_and_status`) + write `session_hibernate`
    ///    resource event. The DB lock (`std::sync::Mutex`) is acquired in a
    ///    scoped block and released before the `.await` on `session.close` —
    ///    never held across an RPC call.
    /// 3. RPC: `session.close(adapter_session_id)` — failure is FATAL (see
    ///    `SidecarTaskExecutor` doc comment).
    async fn evict_session(&self, adapter_session_id: &str) -> anyhow::Result<()> {
        let handle = self
            .supervisor
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
            let harness = self.supervisor.config().harness_name.clone();
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
                // Write memory delta (hot_summary) if present and non-null.
                let mut wrote_summary = false;
                if let Some(delta) = &memory_delta {
                    if !delta.is_null() {
                        let hot_summary = delta
                            .get("hot_summary")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        db_guard
                            .subagent_write_hot_summary(&subagent_id, hot_summary)
                            .map_err(|e| anyhow::anyhow!("write hot_summary failed: {e}"))?;
                        wrote_summary = true;
                    }
                }
                // Atomic: flip binding (is_hot=0, status='closed',
                // closed_at_ms=Some(now)) + logical status='warm'.
                let now = busytok_domain::now_ms();
                let mut flipped = binding.clone();
                flipped.is_hot = 0;
                flipped.status = "closed".into();
                flipped.closed_at_ms = Some(now);
                db_guard
                    .subagent_commit_hibernate_binding_and_status(&flipped, &subagent_id, "warm")
                    .map_err(|e| anyhow::anyhow!("commit hibernate binding failed: {e}"))?;
                // Write `session_hibernate` resource event for observability.
                db_guard
                    .subagent_insert_resource_event(&SubagentResourceEventRow {
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
                    })
                    .map_err(|e| anyhow::anyhow!("insert resource event failed: {e}"))?;
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
    ExecutorOutput {
        adapter_session_id,
        session_reused,
        status,
        summary,
        usage,
    }
}

/// Convert `SidecarError` → `SubagentError` (preserving application error codes)
/// → `anyhow::Error`. The `delegate()` method downcasts back to `SubagentError`
/// so the control contract (`subagent.profile_not_found`, etc.) is honored.
fn sidecar_to_anyhow(e: SidecarError) -> anyhow::Error {
    anyhow::Error::from(SubagentError::from(e))
}
