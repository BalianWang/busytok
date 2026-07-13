//! The public logical-subagent manager.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use busytok_config::SubagentSettings;
use busytok_store::{
    Database, SubagentMemoryRow, SubagentResourceEventRow, SubagentTaskRow,
    SubagentUsageRecordRow,
};
use tracing::{error, info, warn};

use crate::context::ContextBuilder;
use crate::error::{Result, SubagentError};
use crate::memory::MemoryUpdater;
use crate::mock_executor::{ExecutorInput, TaskExecutor};
use crate::models::{
    DelegateRequest, DelegateResult, LogicalSubagent, QueueReason, ResolveParams, SubagentStatus,
    SubagentTaskSummary, TaskErrorKind, TaskStatus, TaskUsage,
};
use crate::pressure::PressureGate;
use crate::resolver::{resolve_by_id, resolve_by_name, row_to_model, Resolved};

type SharedDb = Arc<Mutex<busytok_store::Database>>;

/// RAII guard that removes a task's cancel signal from the `cancel_signals`
/// registry when dropped. This covers all exit paths from `execute_task`:
/// normal return, early return (cancel), and future drop (task abort).
/// Without this, a dropped `execute_task` future leaks the `oneshot::Sender`
/// in the registry forever, and subsequent `cancel_task` calls would find a
/// stale sender that can never signal a non-existent executor.
struct CancelSignalGuard<'a> {
    signals: &'a Mutex<HashMap<String, tokio::sync::oneshot::Sender<()>>>,
    task_id: String,
}

impl Drop for CancelSignalGuard<'_> {
    fn drop(&mut self) {
        self.signals
            .lock()
            .expect("cancel_signals lock poisoned")
            .remove(&self.task_id);
    }
}

/// Buffer (ms) added to the per-task timeout when computing the reaper
/// cutoff: `COALESCE(timeout_seconds, default) * 1000 + REAPER_BUFFER_MS`.
/// Covers non-sidecar overhead (context build, DB writes) so legitimate
/// in-flight tasks are never reaped. Must match the buffer bound in the
/// reaper SQL (`subagent_queries::reap_orphaned_running_tasks`).
const REAPER_BUFFER_MS: i64 = 60_000;

/// Maximum `timeout_seconds` value that will NOT overflow the reaper's
/// `timeout_seconds * 1000 + REAPER_BUFFER_MS` arithmetic in SQLite's
/// 64-bit integer domain. Values above this are rejected at delegate time
/// and capped at the reaper fallback path.
const MAX_SAFE_TIMEOUT_SECONDS: i64 = (i64::MAX - REAPER_BUFFER_MS) / 1000;

/// Maximum wall-clock age (ms) a task may reach before `HotSessionLimit`
/// re-queuing gives up and marks the task as `failed`. When the executor
/// exhausts its in-call retries (`ALL_BUSY_MAX_RETRIES` × `ALL_BUSY_BACKOFF`
/// ≈ 5s) and returns `HotSessionLimit`, `execute_and_persist` flips the task
/// back to `queued` so the dispatcher can retry. This creates a bounded retry
/// loop: each cycle is ~5s of executor retry + 200ms dispatcher tick. At 5
/// minutes the task has had ~60 cycles — enough for any well-behaved
/// concurrent task to finish and free the hot session slot. Past this
/// deadline the task is marked `failed` with `error_kind = hot_session_limit`
/// so it doesn't loop forever.
const HOT_SESSION_RETRY_DEADLINE_MS: i64 = 300_000;

/// Convert a delegate-request `timeout_seconds` override (`u64` from
/// CLI/DTO) to the `i64` representation stored in
/// `subagent_tasks.timeout_seconds`.
///
/// Rejects values that exceed `i64::MAX` or would overflow the reaper's
/// `timeout_seconds * 1000 + REAPER_BUFFER_MS` cutoff arithmetic. This
/// prevents `as i64` truncation from wrapping oversized `u64` values
/// (e.g. `--timeout 9223372036854775808` → `i64::MIN`) to negative, which
/// would make the reaper SQL
/// `started_at_ms < (?1 - (COALESCE(timeout_seconds, ?2) * 1000 + ?3))`
/// evaluate true for healthy tasks, immediately reaping them.
fn validate_timeout_seconds(timeout: u64) -> Result<i64> {
    let secs = i64::try_from(timeout).map_err(|_| {
        SubagentError::InvalidArgument(format!(
            "timeout_seconds {timeout} exceeds i64::MAX ({}); use a smaller value",
            i64::MAX
        ))
    })?;
    secs.checked_mul(1000)
        .and_then(|v| v.checked_add(REAPER_BUFFER_MS))
        .ok_or_else(|| {
            SubagentError::InvalidArgument(format!(
                "timeout_seconds {timeout} would overflow the reaper cutoff arithmetic \
                 (timeout * 1000 + {REAPER_BUFFER_MS}); use a value <= {MAX_SAFE_TIMEOUT_SECONDS}"
            ))
        })?;
    Ok(secs)
}

/// Convert a config-sourced `timeout_seconds` (`u64`) to `i64` for the
/// reaper SQL parameter. Caps to `MAX_SAFE_TIMEOUT_SECONDS` if the value
/// would overflow, ensuring the reaper never crashes on bad config.
/// Config values are trusted but defensively capped — a huge config
/// value would otherwise overflow the SQL `* 1000` arithmetic.
fn cap_timeout_for_reaper(timeout: u64) -> i64 {
    validate_timeout_seconds(timeout).unwrap_or(MAX_SAFE_TIMEOUT_SECONDS)
}

pub struct SubagentManager {
    db: SharedDb,
    settings: SubagentSettings,
    adapter: String,
    executor: Arc<dyn TaskExecutor>,
    context_builder: ContextBuilder,
    memory_updater: MemoryUpdater,
    /// §8.3 step 2: when `Some` and `is_paused()`, `delegate()` inserts the
    /// task row as `'queued'` and returns `DelegateResult { status: Queued }`
    /// instead of executing synchronously. The background `TaskDispatcher`
    /// (Task 7) picks up queued tasks when the gate clears.
    pressure_gate: Option<Arc<PressureGate>>,
    /// Post-task completion hook (P1 #2 fix). When `Some`, the background
    /// dispatcher invokes it after `execute_task()` succeeds so the runtime
    /// can bridge the queued task's usage into the unified `usage_events`
    /// pipeline — the SAME seam as synchronous `delegate()`. Without this,
    /// queued tasks' usage is invisible in Overview / Activity / receipt reads.
    /// The closure receives `(&DelegateResult, cwd)`. Best-effort: the hook
    /// logs failures internally and does NOT propagate errors to the dispatcher.
    /// Set ONCE by `BusytokSupervisor::assemble_with_sidecar` after the
    /// generation manager is constructed.
    task_completion_hook: Mutex<Option<TaskCompletionHook>>,
    /// Per-task cooperative cancel signal registry. When a task starts
    /// executing, a `oneshot::Sender` is registered here; `cancel_task`
    /// signals it so `execute_task` can abort the in-flight executor call
    /// via `tokio::select!`. The entry is removed after execution
    /// completes (success, failure, or cancel).
    cancel_signals: Mutex<HashMap<String, tokio::sync::oneshot::Sender<()>>>,
}

/// Hook invoked by the background dispatcher after a queued task completes
/// successfully. The runtime registers one that calls `bridge_subagent_usage`
/// so queued tasks' usage flows into `usage_events` + rollups.
pub type TaskCompletionHook = Arc<dyn Fn(&DelegateResult, &str) + Send + Sync>;

/// Outcome of a `cancel_task` call. `cancelled == false` when the task was
/// already terminal (completed/failed/cancelled) or the cancel was a no-op.
#[derive(Debug, Clone)]
pub struct CancelOutcome {
    pub previous_status: String,
    pub new_status: String,
    pub cancelled: bool,
}

/// Combined snapshot for `subagent.runtime_status` — all DB reads occur under
/// a single `SubagentManager` lock acquisition to preserve single-read
/// aggregate semantics (spec §4 line 213). The worker sample (collected
/// separately by the supervisor from `PiSidecarSupervisor::worker_snapshot`)
/// is NOT part of this struct — it is stamped with `worker_sampled_at_ms` at
/// the handler layer so consumers know its freshness.
pub struct RuntimeStatusSnapshot {
    /// Active (non-deleted) logical subagents, mapped from raw rows.
    pub subagents: Vec<LogicalSubagent>,
    /// `subagent_id → task_count` (only subagents with ≥1 task appear).
    pub task_counts: std::collections::HashMap<String, u32>,
    /// `subagent_id → (created_at_ms, status_string)` for each subagent's
    /// latest task (only subagents with ≥1 task appear).
    pub last_tasks: std::collections::HashMap<String, (i64, String)>,
    /// Most recent tasks across ALL subagents, newest first (limit-clamped).
    pub recent_tasks: Vec<SubagentTaskSummary>,
    /// `subagent_id → display_name` for ALL subagents (including deleted).
    /// Used by the handler to resolve `tasks_recent[].subagent_name` so the
    /// task history shows display names even for deleted subagents (reviewer
    /// P1-2: decouple display name from delete filtering).
    pub name_lookup: std::collections::HashMap<String, String>,
}

impl SubagentManager {
    pub fn new(
        db: SharedDb,
        settings: SubagentSettings,
        adapter: &str,
        executor: Arc<dyn TaskExecutor>,
    ) -> Self {
        Self::with_pressure_gate(db, settings, adapter, executor, None)
    }

    /// Construct with an explicit `PressureGate`. When the gate is `Some`
    /// and paused, `delegate()` queues the task instead of executing it
    /// (spec §8.3 step 2 "queue only").
    pub fn with_pressure_gate(
        db: SharedDb,
        settings: SubagentSettings,
        adapter: &str,
        executor: Arc<dyn TaskExecutor>,
        pressure_gate: Option<Arc<PressureGate>>,
    ) -> Self {
        let context_builder = ContextBuilder::new(settings.context.clone());
        let memory_updater = MemoryUpdater::new(settings.context.clone());
        Self {
            db,
            settings,
            adapter: adapter.to_string(),
            executor,
            context_builder,
            memory_updater,
            pressure_gate,
            task_completion_hook: Mutex::new(None),
            cancel_signals: Mutex::new(HashMap::new()),
        }
    }

    /// Register a post-task completion hook (P1 #2 fix). The background
    /// dispatcher invokes it after `execute_task()` succeeds so the runtime
    /// can bridge the queued task's usage into `usage_events` + rollups — the
    /// same seam as synchronous `delegate()`. Must be called BEFORE
    /// `spawn_task_dispatcher` (set once by `BusytokSupervisor::assemble_with_sidecar`).
    pub fn set_task_completion_hook(&self, hook: TaskCompletionHook) {
        let mut guard = self
            .task_completion_hook
            .lock()
            .expect("task_completion_hook lock poisoned");
        *guard = Some(hook);
    }

    /// Create-or-continue a subagent and run one task.
    ///
    /// Takes `&Arc<Self>` so execution can be spawned in the background
    /// (Bug #1/#2 fix): `delegate()` inserts the task row and returns
    /// `Running` (or `Queued` under contention) immediately. The spawned
    /// task calls `execute_task` and persists results asynchronously.
    pub async fn delegate(self: &Arc<Self>, req: DelegateRequest) -> Result<DelegateResult> {
        if !self.settings.enabled {
            warn!(
                event_code = "subagent.delegate.rejected",
                reason = "disabled"
            );
            return Err(SubagentError::Disabled);
        }
        // Unknown profile with no model override → fail fast so callers surface
        // configuration typos instead of silently running model-less.
        if req.model_override.is_none() && !self.profile_known(&req.profile) {
            warn!(
                event_code = "subagent.delegate.rejected",
                reason = "profile_not_found",
                profile = %req.profile,
            );
            return Err(SubagentError::ProfileNotFound(req.profile));
        }
        // Spec §4.3: `prompt` and `prompt_artifact_ref` are mutually exclusive.
        // Exactly one must be set (non-empty prompt OR Some artifact ref).
        let has_artifact = req
            .prompt_artifact_ref
            .as_ref()
            .is_some_and(|s| !s.is_empty());
        if has_artifact && !req.prompt.is_empty() {
            warn!(
                event_code = "subagent.delegate.rejected",
                reason = "prompt_and_artifact_ref_both_set"
            );
            return Err(SubagentError::InvalidArgument(
                "prompt and prompt_artifact_ref are mutually exclusive".to_string(),
            ));
        }
        if !has_artifact && req.prompt.is_empty() {
            warn!(
                event_code = "subagent.delegate.rejected",
                reason = "prompt_and_artifact_ref_both_empty"
            );
            return Err(SubagentError::InvalidArgument(
                "either prompt or prompt_artifact_ref must be set".to_string(),
            ));
        }

        // Spec §3.3: bound fields are conditionally required. One without the
        // other is a validation error (`(Some, None)` / `(None, Some)`).
        // `(None, None)` is permitted at this layer for the reuse path (name
        // hit OR `subagent_id` shortcut) — the caller may not know whether the
        // subagent exists. The resolver's creation path (0 active matches)
        // rejects empty bound fields with `SubagentError::Validation`, so a
        // `(None, None)` request that misses the name lookup fails fast with
        // "bound_provider_id and bound_model_id are both required to create a
        // subagent". There is no "create without binding" path (spec §3.3).
        let bound_pair = match (&req.bound_provider_id, &req.bound_model_id) {
            (Some(p), Some(m)) => Some((p.clone(), m.clone())),
            (None, None) => None,
            _ => {
                return Err(SubagentError::Validation(
                    "bound_provider_id and bound_model_id must be provided together".into(),
                ));
            }
        };

        // Validate timeout_seconds BEFORE any DB write: `as i64` truncation of
        // oversized u64 values (e.g. `--timeout 9223372036854775808`) would
        // wrap to negative, corrupting the reaper SQL cutoff and causing
        // healthy tasks to be reaped. Reject early with InvalidArgument.
        let timeout_seconds = req
            .timeout_seconds
            .map(validate_timeout_seconds)
            .transpose()?;

        // 1. resolve subagent (create if needed). `resolve_by_name` canonicalizes
        //    cwd and validates the name; errors propagate with a reject log.
        let Resolved { subagent, created } = {
            let db = self.db.lock().expect("subagent db lock poisoned");
            if let Some(id) = &req.subagent_id {
                // Reuse path: ignore bound fields, resolve by id.
                Resolved {
                    subagent: resolve_by_id(&db, id)?,
                    created: false,
                }
            } else {
                // Name path: pass bound fields; resolver validates only on
                // create (existing subagent → bound fields ignored).
                let (p, m) = bound_pair
                    .clone()
                    .unwrap_or_else(|| (String::new(), String::new()));
                match resolve_by_name(&db, &req.subagent_name, &req.cwd, &req.profile, &p, &m) {
                    Ok(r) => r,
                    Err(e) => {
                        warn!(
                            event_code = "subagent.delegate.rejected",
                            reason = e.code(),
                            name = %req.subagent_name,
                        );
                        return Err(e);
                    }
                }
            }
        };

        // Reuse-policy conflict detection (P1-4 + Bug #3 fix). Enforced
        // after resolution so the resolver's own validation (bound fields
        // required on create, canonical cwd) runs first.
        //
        // Semantics:
        //   create → fail if subagent already exists; otherwise create
        //   reuse  → fail if subagent not found; otherwise reuse
        //   fail   → alias for `create`: fail if subagent already exists
        //   None   → default: create-or-reuse, fail on binding mismatch
        match (&req.reuse_policy.as_deref(), created) {
            (Some("create"), false) | (Some("fail"), false) => {
                return Err(SubagentError::InvalidArgument(format!(
                    "subagent '{}' already exists; use --reuse-policy=reuse to reuse it",
                    req.subagent_name
                )));
            }
            (Some("reuse"), true) => {
                return Err(SubagentError::InvalidArgument(format!(
                    "subagent '{}' not found; use --reuse-policy=create to create it",
                    req.subagent_name
                )));
            }
            // Explicit reuse: proceed without binding conflict check. The
            // user opted into reusing the existing subagent regardless of
            // its current binding.
            (Some("reuse"), false) => {}
            // Default / unknown policy on the reuse path: when the request
            // carries bound fields, verify they match the existing
            // subagent's binding. A mismatch is a configuration error.
            (_, false) => {
                if let (Some(req_pid), Some(req_mid)) =
                    (&req.bound_provider_id, &req.bound_model_id)
                {
                    if req_pid != &subagent.bound_provider_id || req_mid != &subagent.bound_model_id
                    {
                        return Err(SubagentError::InvalidArgument(format!(
                            "subagent '{}' exists but is bound to provider={}/model={}, \
                             which differs from the requested provider={}/model={}. \
                             Use --reuse-policy=reuse to ignore this conflict.",
                            req.subagent_name,
                            subagent.bound_provider_id,
                            subagent.bound_model_id,
                            req_pid,
                            req_mid
                        )));
                    }
                }
            }
            // Create path with any policy other than "reuse" → proceed.
            (_, true) => {}
        }

        // Bug #1/#2 fix: validate bound resources BEFORE inserting the task
        // row. Validation failures (disabled provider, missing api key,
        // missing model metadata) are returned from delegate() immediately
        // rather than being swallowed by the async error handler in
        // execute_and_persist. This preserves the fail-fast contract: callers
        // get a Validation error they can act on, not a silently-failed
        // background task.
        let effective_model_id = req
            .model_override
            .clone()
            .unwrap_or_else(|| subagent.bound_model_id.clone());
        self.validate_bound_resources(&subagent, &effective_model_id)?;

        // 2. insert task row. §8.3 step 2 "queue only" (Round 4 race-free
        //    design) + spec §6.4 per-subagent serialization: check the gate
        //    AND whether this subagent already has a running task BEFORE insert.
        //    - Gate paused OR subagent busy → insert as `"queued"`, return
        //      `Queued` early. The background `TaskDispatcher` (Task 7) picks
        //      it up when the gate clears AND the subagent is free.
        //    - Gate not paused AND subagent free → insert as `"running"`
        //      (with `started_at_ms = now`), spawn execution in the
        //      background, and return `Running` immediately (Bug #1/#2 fix).
        //    The `has_running_task` check + insert happen inside the same DB
        //    lock, so the TOCTOU window is closed (single-process, Rust mutex
        //    serializes all DB access).
        let paused = self
            .pressure_gate
            .as_ref()
            .map(|g| g.is_paused())
            .unwrap_or(false);
        let now = busytok_domain::now_ms();
        let task_id = format!("task_{}", uuid::Uuid::new_v4());
        let should_queue = {
            let db = self.db.lock().expect("subagent db lock poisoned");
            let has_running = db.subagent_has_running_task(&subagent.id).unwrap_or(false);
            let should_queue = paused || has_running;
            db.subagent_insert_task(&SubagentTaskRow {
                id: task_id.clone(),
                subagent_id: subagent.id.clone(),
                source_harness: req.source_harness.clone(),
                source_session_id: req.source_session_id.clone(),
                intent: req.intent.clone(),
                profile: req.profile.clone(),
                prompt: Some(req.prompt.clone()),
                prompt_artifact_ref: req.prompt_artifact_ref.clone(),
                output_schema_name: None,
                output_schema_version: 1,
                status: if should_queue {
                    "queued".to_string()
                } else {
                    "running".to_string()
                },
                result_summary: None,
                result_json: None,
                error: None,
                created_at_ms: now,
                started_at_ms: if should_queue { None } else { Some(now) },
                completed_at_ms: None,
                // Task 7 Round 3 Finding 1 fix: persist execution params so the
                // dispatcher reads them from the row (single source of truth).
                timeout_seconds,
                model_override: req.model_override.clone(),
                error_kind: None,
            })
            .map_err(SubagentError::Store)?;
            // Bug #4 fix: runtime status is computed at read-time in
            // `show()` / `list()` by joining against running tasks — NOT
            // persisted here. Writing `status='hot'` at task start would
            // violate the spec §3.3 invariant ("status='hot' iff is_hot=1
            // binding exists") since no hot binding is created until task
            // completion. On failure, the stale 'hot' would persist with no
            // binding to back it.
            should_queue
        };

        info!(
            event_code = "subagent.delegate.start",
            subagent_id = %subagent.id,
            created,
            profile = %req.profile,
            "delegating task"
        );

        // If should_queue (gate paused OR subagent busy), return Queued — the
        // dispatcher handles it when the gate clears AND the subagent is free.
        // We return immediately without building context or invoking the
        // executor.
        if should_queue {
            info!(
                event_code = "subagent.delegate.queued",
                subagent_id = %subagent.id,
                task_id = %task_id,
                action = ?self.pressure_gate.as_ref().and_then(|g| g.last_action()),
                paused,
                "task queued — gate paused or subagent busy"
            );
            return Ok(DelegateResult {
                task_id,
                subagent_id: subagent.id.clone(),
                subagent_name: subagent.name.clone(),
                adapter: self.adapter.clone(),
                adapter_session_id: None,
                session_reused: false,
                status: TaskStatus::Queued,
                profile: req.profile.clone(),
                model: req.model_override.clone(),
                summary: None,
                usage: TaskUsage::default(),
                created,
                queue_reason: Some(if paused {
                    QueueReason::PressureGatePaused
                } else {
                    QueueReason::SubagentBusy
                }),
            });
        }

        // Bug #1/#2 fix: spawn execution in the background and return
        // `Running` immediately. The caller polls `subagent.task_get` for
        // the final status. `execute_and_persist` handles execution, result
        // persistence, and `task_completion_hook` invocation — the SAME
        // seam used by the background dispatcher.
        let task_row = SubagentTaskRow {
            id: task_id.clone(),
            subagent_id: subagent.id.clone(),
            source_harness: req.source_harness.clone(),
            source_session_id: req.source_session_id.clone(),
            intent: req.intent.clone(),
            profile: req.profile.clone(),
            prompt: Some(req.prompt.clone()),
            prompt_artifact_ref: req.prompt_artifact_ref.clone(),
            output_schema_name: None,
            output_schema_version: 1,
            status: "running".to_string(),
            result_summary: None,
            result_json: None,
            error: None,
            created_at_ms: now,
            started_at_ms: Some(now),
            completed_at_ms: None,
            timeout_seconds,
            model_override: req.model_override.clone(),
            error_kind: None,
        };
        let manager = Arc::clone(self);
        let subagent_clone = subagent.clone();
        tokio::spawn(async move {
            manager.execute_and_persist(&task_row, &subagent_clone).await;
        });

        Ok(DelegateResult {
            task_id,
            subagent_id: subagent.id.clone(),
            subagent_name: subagent.name.clone(),
            adapter: self.adapter.clone(),
            adapter_session_id: None,
            session_reused: false,
            status: TaskStatus::Running,
            profile: req.profile.clone(),
            model: req.model_override.clone(),
            summary: None,
            usage: TaskUsage::default(),
            created,
            queue_reason: None,
        })
    }

    /// Execute a task, persist results, and invoke the `task_completion_hook`.
    /// Shared by `delegate()` (in a spawned task) and the background dispatcher
    /// (inline). This is the single seam for post-execution cleanup: on
    /// success the hook bridges usage into `usage_events` + rollups; on
    /// failure the task is marked `'failed'` in the DB.
    async fn execute_and_persist(
        &self,
        task: &SubagentTaskRow,
        subagent: &LogicalSubagent,
    ) {
        match self.execute_task(task, subagent).await {
            Ok(result) => {
                let hook = {
                    let guard = self
                        .task_completion_hook
                        .lock()
                        .expect("task_completion_hook lock poisoned");
                    guard.as_ref().map(Arc::clone)
                };
                if let Some(hook) = hook {
                    hook(&result, &subagent.repo_path);
                }
            }
            Err(e) => {
                // HotSessionLimit is a transient capacity contention
                // error: all hot sessions were busy and the executor's
                // in-call retries were exhausted. `execute_task` already
                // skipped the `failed` write (Phase 4c) — re-queue the task
                // so the dispatcher can retry when the hot session slot
                // frees up, bounded by `HOT_SESSION_RETRY_DEADLINE_MS` to
                // prevent infinite re-queuing. Past the deadline, mark as
                // `failed` with `error_kind = hot_session_limit`.
                if let SubagentError::HotSessionLimit { .. } = &e {
                    let now = busytok_domain::now_ms();
                    let age_ms = now.saturating_sub(task.created_at_ms);
                    if age_ms < HOT_SESSION_RETRY_DEADLINE_MS {
                        info!(
                            event_code = "subagent.hot_session_limit_requeued",
                            task_id = %task.id,
                            subagent_id = %subagent.id,
                            age_ms,
                            deadline_ms = HOT_SESSION_RETRY_DEADLINE_MS,
                            "HotSessionLimit — re-queuing task for dispatcher retry"
                        );
                        let db = self.db.lock().expect("subagent db lock poisoned");
                        let _ = db.subagent_set_task_status_if_not_cancelled(
                            &task.id,
                            "queued",
                            None,
                            None,
                        );
                        return;
                    }
                    warn!(
                        event_code = "subagent.hot_session_limit_deadline_exceeded",
                        task_id = %task.id,
                        subagent_id = %subagent.id,
                        age_ms,
                        deadline_ms = HOT_SESSION_RETRY_DEADLINE_MS,
                        "HotSessionLimit deadline exceeded — marking task failed"
                    );
                    let db = self.db.lock().expect("subagent db lock poisoned");
                    let _ = db.subagent_set_task_status_if_not_cancelled(
                        &task.id,
                        "failed",
                        None,
                        Some(e.to_string()),
                    );
                    let _ = db
                        .subagent_set_task_error_kind(&task.id, Some(TaskErrorKind::HotSessionLimit.as_str()));
                    return;
                }
                warn!(
                    event_code = "subagent.execute_failed",
                    task_id = %task.id,
                    error = %e,
                    "execute_task failed; marking task failed"
                );
                let db = self.db.lock().expect("subagent db lock poisoned");
                let _ = db.subagent_set_task_status_if_not_cancelled(
                    &task.id,
                    "failed",
                    None,
                    Some(e.to_string()),
                );
            }
        }
    }

    /// Validate the bound provider and model for a task. Returns the resolved
    /// `Provider` and `Model` rows on success, or a `Validation` error on
    /// failure. Called by `delegate()` (before spawning background execution)
    /// so validation failures surface as the `delegate()` return value — not
    /// as a silently-failed background task. Also called by `execute_task()`
    /// (for the dispatcher path, where queued tasks are picked up without
    /// going through `delegate()`).
    ///
    /// The double call (delegate + execute_task) is intentional: state may
    /// change between the two calls (provider disabled, model deleted), and
    /// `execute_task` needs the resolved rows to build `ExecutorInput`.
    fn validate_bound_resources(
        &self,
        subagent: &LogicalSubagent,
        effective_model_id: &str,
    ) -> Result<(busytok_domain::Provider, busytok_domain::Model)> {
        let db = self.db.lock().expect("subagent db lock poisoned");
        let provider = db
            .get_provider_with_secret(&subagent.bound_provider_id)
            .map_err(SubagentError::Store)?
            .ok_or_else(|| {
                SubagentError::Validation(format!(
                    "bound provider not found: {}",
                    subagent.bound_provider_id
                ))
            })?;
        if !provider.enabled {
            return Err(SubagentError::Validation(format!(
                "bound provider disabled: {}",
                subagent.bound_provider_id
            )));
        }
        let api_key = provider.api_key.clone().unwrap_or_default();
        if api_key.is_empty() {
            return Err(SubagentError::Validation(format!(
                "bound provider missing api key: {}",
                subagent.bound_provider_id
            )));
        }
        let model = db
            .get_model_by_provider_and_model_id(
                &subagent.bound_provider_id,
                effective_model_id,
            )
            .map_err(SubagentError::Store)?
            .ok_or_else(|| {
                SubagentError::Validation(format!(
                    "bound model not found in provider: {effective_model_id}"
                ))
            })?;
        if !model.enabled {
            return Err(SubagentError::Validation(format!(
                "bound model disabled: {effective_model_id}"
            )));
        }
        // I-2 fail-fast: context_window + max_tokens are required at
        // execute time. Pre-existing/seed models may have NULL for these
        // columns. unwrap_or(0) would propagate 0 into the Pi SDK's
        // registerProvider, causing silent breakage. Fail fast instead.
        if model.context_window.is_none() {
            return Err(SubagentError::Validation(format!(
                "model '{effective_model_id}' missing context_window metadata; \
                 re-create the model with context_window + max_tokens"
            )));
        }
        if model.max_tokens.is_none() {
            return Err(SubagentError::Validation(format!(
                "model '{effective_model_id}' missing max_tokens metadata; \
                 re-create the model with context_window + max_tokens"
            )));
        }
        Ok((provider, model))
    }

    /// Execute a task that is ALREADY "running" (status + started_at_ms set
    /// by the caller). Builds context → executes → persists results.
    /// Called by `execute_and_persist()` which is shared by `delegate()`
    /// (in a spawned background task) and the background dispatcher (after
    /// `pick_oldest_queued_task` which does the atomic CAS flip).
    ///
    /// **Round 3 Finding 2 fix:** this method does NOT call
    /// `subagent_set_task_status("running")` — the caller sets the status
    /// before calling (via insert-as-running or pick's atomic flip).
    ///
    /// Reads ALL execution params from the task row (Finding 1 fix):
    ///   - task.prompt / task.prompt_artifact_ref
    ///   - task.profile
    ///   - task.timeout_seconds (new column)
    ///   - task.model_override (new column)
    async fn execute_task(
        &self,
        task: &SubagentTaskRow,
        subagent: &LogicalSubagent,
    ) -> Result<DelegateResult> {
        // Spec §4.3: effective model = task.model_override.unwrap_or(bound_model_id)
        let effective_model_id = task
            .model_override
            .clone()
            .unwrap_or_else(|| subagent.bound_model_id.clone());
        // `model` is used in DelegateResult returns (including early cancel
        // returns below). Declared here so all return paths can reference it.
        let model: Option<String> = Some(effective_model_id.clone());

        // Register the cooperative cancel signal at the very start so
        // `cancel_task` can abort execution even during the synchronous
        // setup phase below (provider resolution, context building).
        let (cancel_tx, cancel_rx) = tokio::sync::oneshot::channel::<()>();
        {
            let mut signals = self
                .cancel_signals
                .lock()
                .expect("cancel_signals lock poisoned");
            signals.insert(task.id.clone(), cancel_tx);
        }
        // RAII guard: removes the signal entry from the registry on drop
        // — covers normal return, early return (cancel), and future drop
        // (task abort). Without this, a dropped execute_task future leaks
        // the Sender in the registry forever (M4 fix).
        let _signal_guard = CancelSignalGuard {
            signals: &self.cancel_signals,
            task_id: task.id.clone(),
        };
        // Pre-execute cancel check: `cancel_task` may have flipped the
        // status to "cancelled" in the window between `delegate()`'s task
        // insert and this point (or the dispatcher's pick and this point).
        // Skip the executor call entirely — no tokens consumed (M1 fix).
        {
            let db = self.db.lock().expect("subagent db lock poisoned");
            let already_cancelled = db
                .subagent_get_task(&task.id)
                .ok()
                .flatten()
                .map(|t| t.status == "cancelled")
                .unwrap_or(false);
            if already_cancelled {
                info!(
                    event_code = "subagent.task_cancelled_before_execute",
                    task_id = %task.id,
                    "task was cancelled before execution started; skipping"
                );
                return Ok(DelegateResult {
                    task_id: task.id.clone(),
                    subagent_id: subagent.id.clone(),
                    subagent_name: subagent.name.clone(),
                    adapter: self.adapter.clone(),
                    adapter_session_id: None,
                    session_reused: false,
                    status: TaskStatus::Cancelled,
                    profile: task.profile.clone(),
                    model,
                    summary: Some("cancelled by user".to_string()),
                    created: false,
                    usage: TaskUsage::default(),
                    queue_reason: None,
                });
            }
        }
        // Spec §4.3 validation chain — fail fast on bound provider/model
        // issues. Delegates to `validate_bound_resources` (shared with
        // `delegate()`) so validation logic has a single source of truth.
        let (resolved_provider, resolved_model) =
            self.validate_bound_resources(subagent, &effective_model_id)?;
        let started = busytok_domain::now_ms();
        let (input, memory_row, tasks_since_last_compaction, profile_cfg) = {
            let db = self.db.lock().expect("subagent db lock poisoned");
            let memory_row = db
                .subagent_get_memory(&subagent.id)
                .map_err(SubagentError::Store)?
                .unwrap_or_else(|| SubagentMemoryRow::new_empty(&subagent.id));
            let recent_tasks = db
                .subagent_list_tasks(
                    &subagent.id,
                    self.settings.context.recent_tasks_limit as i64,
                )
                .map_err(SubagentError::Store)?;
            // Authoritative count of tasks since last compaction — NOT derived
            // from recent_tasks.len() (which is capped by recent_tasks_limit).
            let tasks_since_last_compaction = db
                .subagent_count_tasks_since(
                    &subagent.id,
                    memory_row.last_compacted_at_ms.unwrap_or(0),
                )
                .map_err(SubagentError::Store)?;
            let profile_cfg = self.settings.profiles.get(&task.profile);
            let profile_budget = profile_cfg
                .map(|p| p.context_budget_tokens)
                .unwrap_or(self.settings.context.default_budget_tokens);
            let tools = profile_cfg.map(|p| p.tools.clone()).unwrap_or_default();
            let write_access = profile_cfg.map(|p| p.write_access).unwrap_or(false);
            let sub_row = db
                .subagent_get_logical(&subagent.id)
                .map_err(SubagentError::Store)?
                .ok_or_else(|| SubagentError::NotFound(subagent.id.clone()))?;
            // The task row is the single source of truth for the prompt
            // (Finding 1 fix). `prompt` is set when the caller supplied inline
            // text; `prompt_artifact_ref` is set when the caller referenced a
            // stored artifact. The context builder takes the inline prompt.
            let prompt = task.prompt.clone().unwrap_or_default();
            let (compact, snapshot) = self.context_builder.build(
                &sub_row,
                &memory_row,
                &recent_tasks,
                &prompt,
                profile_budget,
            );
            info!(
                event_code = "subagent.context.built",
                subagent_id = %subagent.id,
                budget_tokens = compact.budget_tokens,
                context_chars = compact.compact_context.len(),
                recent_tasks_count = recent_tasks.len(),
                tasks_since_last_compaction,
                "built context for task"
            );
            let input = ExecutorInput {
                subagent_id: subagent.id.clone(),
                subagent_name: subagent.name.clone(),
                // Task 7 brief: `cwd` for `ExecutorInput` is the subagent's
                // canonical `repo_path`. The task was queued FOR this
                // subagent, so the subagent's repo is the right working
                // directory. (The original `delegate()` used `req.cwd`, but
                // `execute_task` takes a `&SubagentTaskRow` which has no
                // `cwd` field — `subagent.repo_path` is the correct
                // substitute and is what `req.cwd` would have been
                // canonicalized to during `resolve_by_name`.)
                cwd: subagent.repo_path.clone(),
                profile: task.profile.clone(),
                model: effective_model_id.clone(),
                prompt,
                prompt_artifact_ref: task.prompt_artifact_ref.clone(),
                timeout_seconds: task.timeout_seconds.map(|t| t as u64),
                tools,
                memory: snapshot,
                context: compact,
                write_access,
                // Task 5: thread the resolved provider config + model
                // metadata end-to-end so the sidecar can route to the
                // correct per-provider supervisor and build a complete model
                // definition (spec §5.2). `provider_id` is now `String`
                // (NOT NULL — validated above), so the WorkerPool always has
                // a route target.
                provider_id: resolved_provider.id.clone(),
                provider_kind: resolved_provider.provider_kind.clone(),
                provider_base_url: resolved_provider.base_url.clone(),
                // 瞬态：不写回 task row，不进日志明文，不进 DTO/response/diagnostic.
                provider_api_key: resolved_provider.api_key.clone().unwrap_or_default(),
                model_reasoning: resolved_model.reasoning,
                // context_window + max_tokens are validated non-null by
                // `validate_bound_resources` above. Using `expect` documents
                // the invariant and provides a clear panic message if the
                // validation contract is ever broken.
                model_context_window: resolved_model
                    .context_window
                    .expect("validated by validate_bound_resources"),
                model_max_tokens: resolved_model
                    .max_tokens
                    .expect("validated by validate_bound_resources"),
                model_display_name: resolved_model.display_name.clone(),
            };
            (
                input,
                memory_row,
                tasks_since_last_compaction,
                profile_cfg.cloned(),
            )
        };
        let out = {
            // The cancel signal was registered at the top of execute_task;
            // `_signal_guard` removes it from the registry on drop (all exit
            // paths). No explicit remove needed in the select! branches.
            //
            // Dropping the executor future stops the CLIENT from awaiting
            // the `turn_auto` RPC response. The execution-protocol cancel
            // (`session.cancel` RPC via `executor.cancel()`, fired by
            // `cancel_task`) aborts the in-flight HTTP request in the
            // sidecar, stopping token generation. However, partial usage
            // from the aborted call is NOT captured — the cancel return
            // path uses `TaskUsage::default()`. This is the accepted
            // tradeoff: the sidecar abort may not return partial usage.
            let exec_fut = self.executor.execute(&input);
            let result = tokio::select! {
                biased;
                _ = cancel_rx => {
                    warn!(
                        event_code = "subagent.task_cancelled_in_flight",
                        task_id = %task.id,
                        "executor aborted by cooperative cancel signal"
                    );
                    // The task status is already `cancelled` in the DB
                    // (set by `cancel_task`). Skip the terminal write
                    // (it would be guarded anyway) and return early.
                    return Ok(DelegateResult {
                        task_id: task.id.clone(),
                        subagent_id: subagent.id.clone(),
                        subagent_name: subagent.name.clone(),
                        adapter: self.adapter.clone(),
                        adapter_session_id: None,
                        session_reused: false,
                        status: TaskStatus::Cancelled,
                        profile: task.profile.clone(),
                        model,
                        summary: Some("cancelled by user".to_string()),
                        created: false,
                        usage: TaskUsage::default(),
                        queue_reason: None,
                    });
                }
                exec_result = exec_fut => {
                    exec_result
                }
            };
            result.map_err(|e| match e.downcast::<SubagentError>() {
                Ok(se) => se,
                Err(other) => {
                    warn!(event_code = "subagent.delegate.executor_failed", error = %other);
                    SubagentError::Store(other)
                }
            })
        };
        let out = match out {
            Ok(out) => out,
            Err(e) => {
                // HotSessionLimit is a transient capacity contention error.
                // Do NOT mark the task as 'failed' here — the caller
                // (`execute_and_persist`) will re-queue the task so the
                // dispatcher can retry when the hot session slot frees up.
                // Returning the error without persisting 'failed' lets the
                // caller decide whether to re-queue or mark as failed (based
                // on the task's age deadline).
                if matches!(e, SubagentError::HotSessionLimit { .. }) {
                    return Err(e);
                }
                // Persist failure state before propagating. The executor
                // classified the error (auth/rate_limit/network/timeout) and
                // already killed the worker on auth-fail — but since
                // execute() returns Err, the success-path block below
                // (which persists error_kind) is unreachable. Persist
                // status="failed" + best-effort error_kind here so the UI
                // can surface the failure reason (spec §3.4, Task 5).
                let error_kind = subagent_error_to_task_error_kind(&e);
                {
                    let db = self.db.lock().expect("subagent db lock poisoned");
                    // Use the guarded variant so a late cancel is not
                    // overwritten by this failure write.
                    db.subagent_set_task_status_if_not_cancelled(&task.id, "failed", None, None)
                        .ok();
                    if let Some(kind) = error_kind {
                        if let Err(persist_err) =
                            db.subagent_set_task_error_kind(&task.id, Some(kind.as_str()))
                        {
                            warn!(
                                event_code = "subagent.error_kind_persist_failed",
                                task_id = %task.id,
                                error = %persist_err,
                                "failed to persist error_kind on Err path"
                            );
                        }
                    }
                }
                return Err(e);
            }
        };
        let duration_ms = busytok_domain::now_ms().saturating_sub(started);

        // 4. persist results: task status, usage, memory, status.
        //    For the hot path (real adapter_session_id), the binding + status
        //    flip commit in a single transaction so the spec §3.3 invariant
        //    ("status='hot' iff is_hot=1 binding exists") holds at every
        //    observable point.
        {
            let db = self.db.lock().expect("subagent db lock poisoned");
            // Use the guarded variant so a late cancel is not overwritten
            // by the executor's terminal write (P1-5 review finding C1).
            let updated = db
                .subagent_set_task_status_if_not_cancelled(
                    &task.id,
                    out.status.as_str(),
                    Some(out.summary.clone()),
                    None,
                )
                .map_err(SubagentError::Store)?;
            if !updated {
                info!(
                    event_code = "subagent.task_terminal_write_skipped",
                    task_id = %task.id,
                    attempted_status = out.status.as_str(),
                    "terminal write skipped: task was already cancelled"
                );
            }
            // Task 5: persist classified error_kind on the task row.
            if let Some(kind) = &out.error_kind {
                if let Err(e) = db.subagent_set_task_error_kind(&task.id, Some(kind.as_str())) {
                    tracing::warn!(
                        event_code = "subagent.error_kind_persist_failed",
                        task_id = %task.id,
                        error = %e,
                        "failed to persist error_kind"
                    );
                }
            }
            // Re-fetch recent_tasks AFTER the task result is persisted so the
            // snapshot includes the just-completed task's result_summary. The
            // pre-execution snapshot (used only for context building above)
            // has result_summary=None for the current task, which would make
            // the attempts logic skip the entry and exclude the most recent
            // task from compaction's "Recent findings" section.
            let recent_tasks = db
                .subagent_list_tasks(
                    &subagent.id,
                    self.settings.context.recent_tasks_limit as i64,
                )
                .map_err(SubagentError::Store)?;
            db.subagent_insert_usage_record(&SubagentUsageRecordRow {
                id: format!("usage_{}", task.id),
                task_id: task.id.clone(),
                subagent_id: subagent.id.clone(),
                source_usage_event_id: None,
                harness: self.adapter.clone(),
                provider: out.usage.provider.clone(),
                model: out.usage.model.clone(),
                input_tokens: out.usage.input_tokens,
                output_tokens: out.usage.output_tokens,
                cache_read_tokens: out.usage.cache_read_tokens,
                cache_write_tokens: out.usage.cache_write_tokens,
                total_cost_usd: out.usage.cost_usd,
                duration_ms: Some(duration_ms),
                created_at_ms: busytok_domain::now_ms(),
            })
            .map_err(SubagentError::Store)?;

            // memory: merge the sidecar's memory_update into the memory row
            // (spec §6.2). hot_summary comes from current_state_summary, NOT
            // task_summary. When memory_update is absent, hot_summary is
            // preserved. Compaction runs if triggers fire.
            let profile_budget = profile_cfg
                .as_ref()
                .map(|p| p.context_budget_tokens)
                .unwrap_or(self.settings.context.default_budget_tokens);
            let prev_compacted_at = memory_row.last_compacted_at_ms;
            let updated_mem = self.memory_updater.update(
                memory_row,
                out.memory_update.clone(),
                &recent_tasks,
                tasks_since_last_compaction,
                &task.id,
                profile_budget,
                &subagent.repo_path,
            );
            let compacted = updated_mem.last_compacted_at_ms != prev_compacted_at;
            if compacted {
                info!(
                    event_code = "subagent.memory.compacted",
                    subagent_id = %subagent.id,
                    tasks_since_last_compaction,
                    "memory compacted (trigger fired)"
                );
            }
            info!(
                event_code = "subagent.memory.updated",
                subagent_id = %subagent.id,
                has_hot_summary = updated_mem.hot_summary.is_some(),
                compacted,
                "memory updated after task"
            );
            db.subagent_upsert_memory(&updated_mem)
                .map_err(SubagentError::Store)?;

            // Spec §3.3 invariant: status='hot' iff is_hot=1 binding exists.
            // Hot path: commit binding + status atomically; failure fails the
            // task to preserve the status invariant (no `hot` without a
            // backing binding). Warm path (mock executor, no adapter_session_id):
            // just flip status — no real session to bind.
            //
            // An empty `adapter_session_id` is treated as warm/cold — there is
            // no real backing session, so committing a hot binding would
            // violate spec §3.3 semantically. The task is the authority
            // that decides hot vs warm/cold (the executor only extracts the
            // raw value).
            let hot_sid = out.adapter_session_id.as_deref().filter(|s| !s.is_empty());
            if let Some(sid) = hot_sid {
                let now_ms = busytok_domain::now_ms();
                let binding = busytok_store::repository::SubagentHarnessBindingRow {
                    id: uuid::Uuid::new_v4().to_string(),
                    subagent_id: subagent.id.clone(),
                    harness: self.adapter.clone(),
                    adapter_session_id: Some(sid.to_string()),
                    adapter_process_id: None, // Plan 3 tracks PID
                    is_hot: 1,
                    status: "hot".to_string(),
                    created_at_ms: now_ms,
                    last_used_at_ms: Some(now_ms),
                    closed_at_ms: None,
                    detail_json: None,
                };
                db.subagent_commit_hot_binding_and_status(&binding, &subagent.id)
                    .map_err(|e| {
                        error!(
                            event_code = "subagent.delegate.binding_commit_failed",
                            error = %e,
                            "hot binding commit failed; task fails to preserve status invariant"
                        );
                        SubagentError::Store(e)
                    })?;
            } else {
                // Mock executor path OR empty adapter_session_id — no real
                // session to bind. Derive warm/cold from memory state (§3.3:
                // warm iff hot_summary IS NOT NULL). On a fresh subagent with
                // no memory_update, hot_summary is None → Cold.
                let status = if updated_mem.hot_summary.is_some() {
                    SubagentStatus::Warm
                } else {
                    SubagentStatus::Cold
                };
                self.set_logical_status(&db, &subagent.id, status)?;
            }
        }

        // M2 fix: After persisting usage/memory/binding (tokens were consumed,
        // learnings saved), check if the task was cancelled concurrently.
        // If so, return `Cancelled` for consistency with the DB status, but
        // preserve the REAL usage data so the audit trail is accurate.
        // Without this, `delegate()` would return `Completed` while the DB
        // says `cancelled` — an inconsistent return value.
        let final_status = {
            let db = self.db.lock().expect("subagent db lock poisoned");
            db.subagent_get_task(&task.id)
                .ok()
                .flatten()
                .map(|t| {
                    if t.status == "cancelled" {
                        TaskStatus::Cancelled
                    } else {
                        out.status
                    }
                })
                .unwrap_or(out.status)
        };
        if final_status == TaskStatus::Cancelled {
            warn!(
                event_code = "subagent.task_cancelled_after_complete",
                task_id = %task.id,
                "executor completed but task was cancelled concurrently; usage persisted, returning Cancelled"
            );
        }

        Ok(DelegateResult {
            task_id: task.id.clone(),
            subagent_id: subagent.id.clone(),
            subagent_name: subagent.name.clone(),
            adapter: self.adapter.clone(),
            adapter_session_id: out.adapter_session_id.clone(),
            session_reused: out.session_reused,
            status: final_status,
            profile: task.profile.clone(),
            model,
            summary: Some(out.summary),
            usage: out.usage.clone(),
            // `created` is set by the caller (`delegate` or the dispatcher);
            // default to `false` here (the dispatcher path always reuses).
            created: false,
            queue_reason: None,
        })
    }

    /// Reap orphaned `running` tasks: mark as `failed` any task whose
    /// `started_at_ms` is older than `max_profile_timeout + buffer`.
    ///
    /// Runs on the dispatcher's 30s reaper cadence. Recovers from
    /// `dispatch_timeout` orphans (control-server timeout drops the
    /// `execute_task` future without persisting `status='failed'`),
    /// panics, and crashes — any `running` row whose age exceeds the
    /// ceiling is flipped to `failed` with `error='ORPHANED_REAPED'`.
    /// This unblocks `pick_oldest_queued_task` (which excludes
    /// subagents with a running task) so queued delegates proceed.
    fn reap_orphaned_tasks(&self) {
        // default_timeout = max(profile timeouts, pi_sidecar.task_timeout).
        // Used as the fallback when a task row has no `timeout_seconds`
        // (NULL). Per-task timeout overrides are honored in SQL via
        // COALESCE(timeout_seconds, default_timeout_seconds).
        let max_profile = self
            .settings
            .profiles
            .values()
            .map(|p| p.timeout_seconds)
            .max()
            .unwrap_or(300);
        let sidecar_timeout = self.settings.pi_sidecar.task_timeout_seconds;
        let default_timeout = max_profile.max(sidecar_timeout);
        let buffer_ms = REAPER_BUFFER_MS;
        let now_ms = busytok_domain::now_ms();
        let reaped = {
            let db = self.db.lock().expect("subagent db lock poisoned");
            db.subagent_reap_orphaned_running_tasks(
                now_ms,
                cap_timeout_for_reaper(default_timeout),
                buffer_ms,
            )
            .unwrap_or_else(|e| {
                warn!(
                    event_code = "subagent.reaper.failed",
                    error = %e,
                    "reaper scan failed"
                );
                Vec::new()
            })
        };
        if !reaped.is_empty() {
            warn!(
                event_code = "subagent.reaper.reaped",
                count = reaped.len(),
                task_ids = ?reaped,
                default_timeout_seconds = default_timeout,
                buffer_ms,
                "reaped orphaned running tasks (likely dispatch_timeout orphans)"
            );
        }
    }

    /// Spawn the background task dispatcher (§8.3 step 2 "queue only").
    /// Polls for queued tasks every 200ms; when the gate is not paused,
    /// picks the oldest queued task and executes it. Terminates when
    /// `shutdown` receiver sees `true` (sent by `BusytokSupervisor` on
    /// drop/shutdown).
    ///
    /// **Finding 3 fix:** `JoinHandle` drop = detach (NOT abort), so we use
    /// `tokio::sync::watch` for explicit shutdown signaling. The caller
    /// MUST keep the `Sender` alive and send `true` to stop the dispatcher.
    pub fn spawn_task_dispatcher(
        self: &Arc<Self>,
        mut shutdown: tokio::sync::watch::Receiver<bool>,
    ) -> tokio::task::JoinHandle<()> {
        let manager = Arc::clone(self);
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(Duration::from_millis(200));
            // Reaper runs on its own slower cadence (every 30s) — orphan
            // detection doesn't need 200ms latency, and the scan touches
            // every `running` row.
            let mut reaper_ticker = tokio::time::interval(Duration::from_secs(30));
            reaper_ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    _ = ticker.tick() => {},
                    _ = reaper_ticker.tick() => {
                        manager.reap_orphaned_tasks();
                        continue;
                    }
                    _ = shutdown.changed() => {
                        if *shutdown.borrow() {
                            info!(
                                event_code = "subagent.dispatcher.shutdown",
                                "task dispatcher shutting down"
                            );
                            return;
                        }
                    }
                }

                // Check gate — if paused, skip this tick.
                if let Some(gate) = &manager.pressure_gate {
                    if gate.is_paused() {
                        continue;
                    }
                }

                // Pick oldest queued task (per-subagent FIFO, atomic pick + flip).
                // `pick_oldest_queued_task` returns `None` when no queued task
                // is eligible (no queued rows OR the only queued rows belong to
                // subagents that already have a running task — per-subagent FIFO).
                let task = {
                    let db = manager.db.lock().expect("subagent db lock poisoned");
                    db.subagent_pick_oldest_queued_task().ok().flatten()
                };

                if let Some(task) = task {
                    // Resolve the subagent from the DB so we have the
                    // canonical `repo_path`, `bound_provider_id`, etc.
                    let subagent = {
                        let db = manager.db.lock().expect("subagent db lock poisoned");
                        db.subagent_get_logical(&task.subagent_id)
                            .ok()
                            .flatten()
                            .map(|row| row_to_model(&row))
                    };
                    let Some(subagent) = subagent else {
                        warn!(
                            event_code = "subagent.queue.subagent_missing",
                            task_id = %task.id,
                            subagent_id = %task.subagent_id,
                            "dispatcher could not resolve subagent for queued task; marking failed"
                        );
                        let db = manager.db.lock().expect("subagent db lock poisoned");
                        let _ = db.subagent_set_task_status_if_not_cancelled(
                            &task.id,
                            "failed",
                            None,
                            Some("subagent not found".to_string()),
                        );
                        continue;
                    };
                    info!(
                        event_code = "subagent.queue.execute",
                        task_id = %task.id,
                        subagent_id = %subagent.id,
                        "dispatcher picked queued task for execution"
                    );
                    // Pre-execute cancel re-check: the task may have been
                    // cancelled between `pick` and now. Re-read the status
                    // to avoid starting execution on a cancelled task.
                    let cancelled_before_execute = {
                        let db = manager.db.lock().expect("subagent db lock poisoned");
                        db.subagent_get_task(&task.id)
                            .ok()
                            .flatten()
                            .map(|t| t.status == "cancelled")
                            .unwrap_or(true)
                    };
                    if cancelled_before_execute {
                        info!(
                            event_code = "subagent.queue.skip_cancelled",
                            task_id = %task.id,
                            "skipping execution: task was cancelled while queued"
                        );
                        continue;
                    }
                    // Bug #4 fix: runtime status is computed at read-time in
                    // `show()` / `list()` — no persistent write here. See
                    // `delegate()` for the rationale.
                    // Shared execute + persist + hook seam (same as delegate()).
                    manager.execute_and_persist(&task, &subagent).await;
                }
            }
        })
    }

    /// Count subagent tasks by status. Returns `(queued, running)`. Used by
    /// `ResourceMonitor` (via `PiSidecarSupervisor`) to populate the
    /// `queued_task_count` / `running_task_count` fields of `ResourceSample`
    /// (spec §8.1). Errors are logged at the caller; this method returns
    /// `(0, 0)` on DB failure so a transient lock error doesn't crash the
    /// supervision loop.
    pub fn task_counts(&self) -> (u32, u32) {
        let db = self.db.lock().expect("subagent db lock poisoned");
        db.subagent_task_counts_by_status().unwrap_or((0, 0))
    }

    /// Get a task's current status string by ID. Returns `None` if the task
    /// doesn't exist. Used by callers (and tests) that need to poll for
    /// task completion without resolving the parent subagent.
    pub fn task_status(&self, task_id: &str) -> Result<Option<String>> {
        let db = self.db.lock().expect("subagent db lock poisoned");
        let task = db
            .subagent_get_task(task_id)
            .map_err(SubagentError::Store)?;
        Ok(task.map(|t| t.status))
    }

    /// Cancel a task by id. Idempotent on terminal states: returns
    /// `cancelled == false` when the task was already completed/failed/
    /// cancelled. Returns an error when the task is not found.
    pub async fn cancel_task(&self, task_id: &str, reason: Option<&str>) -> Result<CancelOutcome> {
        let now_ms = busytok_domain::now_ms();
        let task = {
            let db = self.db.lock().expect("subagent db lock poisoned");
            db.subagent_get_task(task_id)
                .map_err(SubagentError::Store)?
                .ok_or_else(|| SubagentError::NotFound(format!("task {task_id}")))?
        };
        let previous_status = task.status.clone();
        if matches!(
            previous_status.as_str(),
            "completed" | "failed" | "cancelled"
        ) {
            return Ok(CancelOutcome {
                previous_status: previous_status.clone(),
                new_status: previous_status,
                cancelled: false,
            });
        }
        let updated = {
            let db = self.db.lock().expect("subagent db lock poisoned");
            db.subagent_cancel_task_if_not_terminal(task_id, reason, now_ms)
                .map_err(SubagentError::Store)?
        };
        if !updated {
            // Lost a race — another writer flipped the task to a terminal
            // state between our read and the conditional UPDATE. Re-read
            // the actual current status so `new_status` is accurate.
            let actual_status = {
                let db = self.db.lock().expect("subagent db lock poisoned");
                db.subagent_get_task(task_id)
                    .map_err(SubagentError::Store)?
                    .map(|t| t.status)
                    .unwrap_or_else(|| previous_status.clone())
            };
            return Ok(CancelOutcome {
                previous_status: previous_status.clone(),
                new_status: actual_status,
                cancelled: false,
            });
        }
        info!(
            event_code = "subagent.task_cancelled",
            task_id = %task_id,
            previous_status = %previous_status,
            reason = ?reason,
            "task cancelled"
        );
        // Signal the in-flight executor to abort (cooperative cancel).
        // If the task is queued (not yet executing), no signal is
        // registered and this is a no-op — the dispatcher's pre-execute
        // re-check will catch the cancelled status.
        if let Some(tx) = {
            let mut signals = self
                .cancel_signals
                .lock()
                .expect("cancel_signals lock poisoned");
            signals.remove(task_id)
        } {
            let _ = tx.send(());
            info!(
                event_code = "subagent.task_cancel_signal_sent",
                task_id = %task_id,
                "cooperative cancel signal sent to in-flight executor"
            );
        }
        // Execution-protocol cancel: send `session.cancel` RPC to the sidecar
        // so it aborts the in-flight HTTP request to the LLM provider —
        // stopping token generation. This is the execution-layer counterpart
        // to the local signal above. The local signal drops the executor
        // future (so the Rust side stops waiting); the sidecar cancel
        // actually stops the model call.
        //
        // Fire-and-forget with a 5s timeout: we don't want to block
        // `cancel_task` on a potentially slow RPC. The DB status is already
        // `cancelled` regardless of this call's outcome. Failures are logged
        // but don't change the cancel result.
        let subagent_id = task.subagent_id.clone();
        let provider_id = {
            let db = self.db.lock().expect("subagent db lock poisoned");
            db.subagent_get_logical(&task.subagent_id)
                .ok()
                .flatten()
                .map(|s| s.bound_provider_id)
                .unwrap_or_default()
        };
        if !provider_id.is_empty() {
            let executor = std::sync::Arc::clone(&self.executor);
            let tid = task_id.to_string();
            tokio::spawn(async move {
                let cancel_fut = executor.cancel(&subagent_id, &provider_id);
                match tokio::time::timeout(std::time::Duration::from_secs(5), cancel_fut).await {
                    Ok(Ok(())) => info!(
                        event_code = "subagent.sidecar_cancel_acknowledged",
                        task_id = %tid,
                        subagent_id = %subagent_id,
                        provider_id = %provider_id,
                        "sidecar acknowledged cancel — underlying model call aborted"
                    ),
                    Ok(Err(e)) => warn!(
                        event_code = "subagent.sidecar_cancel_failed",
                        task_id = %tid,
                        subagent_id = %subagent_id,
                        error = %e,
                        "sidecar cancel failed — underlying model call may continue"
                    ),
                    Err(_) => warn!(
                        event_code = "subagent.sidecar_cancel_timeout",
                        task_id = %tid,
                        subagent_id = %subagent_id,
                        "sidecar cancel timed out (5s) — underlying model call may continue"
                    ),
                }
            });
        }
        Ok(CancelOutcome {
            previous_status,
            new_status: "cancelled".to_string(),
            cancelled: true,
        })
    }

    /// List subagents, optionally filtered by status / project / include-deleted.
    pub async fn list(
        &self,
        status: Option<SubagentStatus>,
        project: Option<&str>,
        include_deleted: bool,
    ) -> Result<Vec<LogicalSubagent>> {
        let db = self.db.lock().expect("subagent db lock poisoned");
        // Bug fix: for Hot/Warm/Cold filters, do NOT push the status filter
        // into SQL. The runtime overlay (which promotes subagents with running
        // tasks to `hot`) must run BEFORE the status filter, otherwise
        // `list --status hot` misses subagents whose DB status is cold/warm
        // but have an in-flight task. For `Deleted`, the overlay is irrelevant
        // (deleted subagents have no running tasks), so the SQL filter is safe.
        let db_status_filter = if matches!(status, Some(SubagentStatus::Deleted)) {
            status.map(|s| s.as_str())
        } else {
            None
        };
        let rows = db
            .subagent_list_filtered(db_status_filter, project, include_deleted)
            .map_err(SubagentError::Store)?;
        let mut subagents: Vec<LogicalSubagent> =
            rows.iter().map(row_to_model).collect::<Vec<_>>();
        // Bug #4 fix: overlay runtime status at read-time. A subagent with an
        // in-flight task is presented as `hot` even if no hot binding exists
        // yet — this is a display-time computation, NOT a persistent write,
        // so the spec §3.3 invariant ("status='hot' iff is_hot=1 binding
        // exists") is preserved in the DB.
        self.overlay_runtime_status(&db, &mut subagents)?;
        // Apply the requested status filter in Rust (post-overlay) so that
        // runtime-promoted subagents are correctly included/excluded.
        if let Some(filter) = status {
            if !matches!(filter, SubagentStatus::Deleted) {
                subagents.retain(|s| s.status == filter);
            }
        }
        Ok(subagents)
    }

    pub async fn show(&self, resolve: ResolveParams) -> Result<LogicalSubagent> {
        let db = self.db.lock().expect("subagent db lock poisoned");
        let mut sub = self.resolve(&db, &resolve)?;
        // Bug #4 fix: overlay runtime status so `show` reflects `hot` while a
        // task is in-flight, without persisting `status='hot'` (which would
        // violate the §3.3 invariant). See `overlay_runtime_status`.
        self.overlay_runtime_status(&db, std::slice::from_mut(&mut sub))?;
        Ok(sub)
    }

    /// Bug #4 fix: overlay runtime status onto a slice of logical subagents
    /// at read-time. For each subagent that has a running task, override the
    /// displayed `status` to `Hot` and set `last_active_at_ms` to `now` (if
    /// not already set or older than the task's start).
    ///
    /// This is a DISPLAY-TIME computation — the DB row is NOT modified. The
    /// spec §3.3 invariant ("status='hot' iff is_hot=1 binding exists") is
    /// preserved in persistent storage. The override only affects what
    /// `show()` / `list()` return to callers.
    ///
    /// Rationale: persisting `status='hot'` at task start (the previous fix)
    /// broke the invariant because no hot binding exists until task
    /// completion. On task failure, the stale `hot` would persist with no
    /// binding to back it. Computing at read-time avoids this entirely.
    fn overlay_runtime_status(
        &self,
        db: &Database,
        subagents: &mut [LogicalSubagent],
    ) -> Result<()> {
        if subagents.is_empty() {
            return Ok(());
        }
        let now = busytok_domain::now_ms();
        for sub in subagents.iter_mut() {
            let has_running = db
                .subagent_has_running_task(&sub.id)
                .map_err(|e| SubagentError::Store(anyhow::Error::new(e)))?;
            if has_running {
                sub.status = SubagentStatus::Hot;
                // Refresh last_active_at_ms to now if it was null or stale
                // (older than 5 minutes — avoids refreshing on every poll).
                let needs_refresh = sub
                    .last_active_at_ms
                    .map(|ts| now - ts > 300_000)
                    .unwrap_or(true);
                if needs_refresh {
                    sub.last_active_at_ms = Some(now);
                }
            }
        }
        Ok(())
    }

    pub async fn tasks(
        &self,
        resolve: ResolveParams,
        limit: i64,
    ) -> Result<Vec<SubagentTaskSummary>> {
        // Clamp and validate limit — SQLite treats negative LIMIT as "no limit",
        // which would bypass the "recent N tasks" boundary.
        const MAX_TASKS_LIMIT: i64 = 500;
        if limit < 0 {
            return Err(SubagentError::InvalidArgument(format!(
                "limit must be >= 0, got {limit}"
            )));
        }
        let limit = limit.min(MAX_TASKS_LIMIT);
        let db = self.db.lock().expect("subagent db lock poisoned");
        let sub = self.resolve(&db, &resolve)?;
        let rows = db
            .subagent_list_tasks(&sub.id, limit)
            .map_err(SubagentError::Store)?;
        Ok(rows.into_iter().map(task_row_to_summary).collect())
    }

    /// Returns the most recent tasks across ALL subagents, newest first
    /// (spec §4 Phase 2 `tasks_recent`). `limit` is passed through to the
    /// store layer; callers (e.g. Task 6's `runtime_status_snapshot`) are
    /// responsible for clamping. The mapping reuses `task_row_to_summary`
    /// so the shape matches `tasks()`.
    pub async fn recent_tasks_all(&self, limit: i64) -> Result<Vec<SubagentTaskSummary>> {
        let db = self.db.lock().expect("subagent db lock poisoned");
        let rows = db
            .subagent_list_recent_tasks_all(limit)
            .map_err(SubagentError::Store)?;
        Ok(rows.into_iter().map(task_row_to_summary).collect())
    }

    /// Returns a map of `subagent_id → task_count` (spec §4 Phase 2
    /// `subagents[].task_count`). Only subagents with at least one task
    /// appear; the underlying query groups by `subagent_id`.
    pub async fn task_counts_by_subagent(&self) -> Result<std::collections::HashMap<String, u32>> {
        let db = self.db.lock().expect("subagent db lock poisoned");
        let counts = db
            .subagent_count_tasks_by_subagent()
            .map_err(SubagentError::Store)?;
        Ok(counts.into_iter().collect())
    }

    /// Returns a map of `subagent_id → (created_at_ms, status)` for each
    /// subagent's latest task (spec §4 Phase 2 `subagents[].last_task_{created_at,
    /// status}`). Only subagents with at least one task appear.
    pub async fn last_task_by_subagent(
        &self,
    ) -> Result<std::collections::HashMap<String, (i64, String)>> {
        let db = self.db.lock().expect("subagent db lock poisoned");
        let lasts = db
            .subagent_last_task_by_subagent()
            .map_err(SubagentError::Store)?;
        Ok(lasts
            .into_iter()
            .map(|(sub_id, created_at, status)| (sub_id, (created_at, status)))
            .collect())
    }

    /// Combined snapshot for `subagent.runtime_status` — performs all 4 DB
    /// reads (`subagent_list_filtered`, `subagent_count_tasks_by_subagent`,
    /// `subagent_last_task_by_subagent`, `subagent_list_recent_tasks_all`)
    /// under a single DB lock acquisition to preserve single-read aggregate
    /// semantics (spec §4 line 213). Avoids 4 separate lock acquisitions that
    /// could observe inconsistent DB state.
    ///
    /// `recent_limit` is passed through to the store layer; callers are
    /// responsible for clamping. The `subagents` vector excludes deleted
    /// rows (filtered in Rust after querying with `include_deleted=true`
    /// so `name_lookup` can include deleted subagent names), matching the
    /// `list()` default for the `subagents[]` DTO.
    pub async fn runtime_status_snapshot(
        &self,
        recent_limit: i64,
    ) -> Result<RuntimeStatusSnapshot> {
        let db = self.db.lock().expect("subagent db lock poisoned");
        // Query ALL subagents (include_deleted=true) once, then split:
        //  - `name_lookup` from ALL rows (id → name) so tasks_recent can
        //    resolve display names even for deleted subagents (reviewer P1-2).
        //  - `subagents` DTO from non-deleted rows only.
        let all_rows = db
            .subagent_list_filtered(None, None, true)
            .map_err(SubagentError::Store)?;
        let name_lookup: std::collections::HashMap<String, String> = all_rows
            .iter()
            .map(|r| (r.id.clone(), r.name.clone()))
            .collect();
        let subagents: Vec<LogicalSubagent> = all_rows
            .iter()
            .filter(|r| r.status != "deleted")
            .map(row_to_model)
            .collect();
        // Bug #4 fix: apply the same runtime-status overlay used by `show()` /
        // `list()` so the monitoring page's `subagents[].status` is consistent
        // with the per-subagent read paths. Without this, a subagent with an
        // in-flight task would show as `cold`/`warm` on the monitoring page
        // while `show()` shows `hot`.
        let mut subagents = subagents;
        self.overlay_runtime_status(&db, &mut subagents)?;
        let task_counts: std::collections::HashMap<String, u32> = db
            .subagent_count_tasks_by_subagent()
            .map_err(SubagentError::Store)?
            .into_iter()
            .collect();
        let last_tasks: std::collections::HashMap<String, (i64, String)> = db
            .subagent_last_task_by_subagent()
            .map_err(SubagentError::Store)?
            .into_iter()
            .map(|(sub_id, created_at, status)| (sub_id, (created_at, status)))
            .collect();
        let recent_tasks: Vec<SubagentTaskSummary> = db
            .subagent_list_recent_tasks_all(recent_limit)
            .map_err(SubagentError::Store)?
            .into_iter()
            .map(task_row_to_summary)
            .collect();
        Ok(RuntimeStatusSnapshot {
            subagents,
            task_counts,
            last_tasks,
            recent_tasks,
            name_lookup,
        })
    }

    /// Release any hot binding for this subagent; keep DB state (warm/cold).
    /// Returns the resolved subagent id so callers can echo it back.
    pub async fn hibernate(&self, resolve: ResolveParams) -> Result<String> {
        let db = self.db.lock().expect("subagent db lock poisoned");
        let sub = self.resolve(&db, &resolve)?;
        // Compute new_status (warm/cold based on memory) BEFORE the transaction.
        // The status follows the invariant: memory exists → warm, else cold.
        let new_status = match db
            .subagent_get_memory(&sub.id)
            .map_err(SubagentError::Store)?
            .and_then(|m| m.hot_summary)
        {
            Some(_) => SubagentStatus::Warm,
            None => SubagentStatus::Cold,
        };
        let binding = db
            .subagent_hot_binding(&sub.id, &self.adapter)
            .map_err(SubagentError::Store)?;
        if let Some(mut b) = binding {
            let now = busytok_domain::now_ms();
            b.is_hot = 0;
            b.status = "closed".to_string();
            b.closed_at_ms = Some(now);
            // Spec §3.3 invariant: status='hot' iff is_hot=1 binding exists.
            // Commit the binding flip (is_hot=0, status='closed') AND the
            // logical status flip (warm/cold) in a single transaction so
            // readers never observe `status='hot'` with no `is_hot=1` binding.
            // Mirrors the delegate path's `commit_hot_binding_and_status`.
            db.subagent_commit_hibernate_binding_and_status(&b, &sub.id, new_status.as_str())
                .map_err(|e| {
                    error!(
                        event_code = "subagent.session.hibernate_commit_failed",
                        error = %e,
                        "hibernate binding+status commit failed; invariant may be violated"
                    );
                    SubagentError::Store(e)
                })?;
            // Resource event is observational (audit), not invariant-critical,
            // so it stays OUTSIDE the transaction. If this insert fails the
            // §3.3 invariant still holds — we only lose an audit row.
            db.subagent_insert_resource_event(&SubagentResourceEventRow {
                id: format!("re_{}", uuid::Uuid::new_v4()),
                event_type: "session_hibernate".to_string(),
                target_id: Some(sub.id.clone()),
                rss_mb: None,
                cpu_percent: None,
                detail_json: None,
                created_at_ms: now,
            })
            .map_err(SubagentError::Store)?;
            info!(event_code = "subagent.session.hibernate", subagent_id = %sub.id, "hibernated hot session");
        } else {
            // No hot binding to release — just flip the logical status. The
            // §3.3 invariant already holds (no is_hot=1 binding exists), so a
            // non-atomic status flip is safe here.
            self.set_logical_status(&db, &sub.id, new_status)?;
            info!(
                event_code = "subagent.session.hibernate_noop",
                subagent_id = %sub.id,
                status = %new_status.as_str(),
                "no hot binding; flipped logical status"
            );
        }
        Ok(sub.id)
    }

    /// Soft delete (default) or hard delete with `hard=true`.
    /// Returns the resolved subagent id so callers can echo it back.
    /// Hard delete may operate on already-tombstoned rows; soft delete on a
    /// tombstone is a no-op success.
    pub async fn delete(&self, resolve: ResolveParams, hard: bool) -> Result<String> {
        let db = self.db.lock().expect("subagent db lock poisoned");
        // Hard delete needs to reach tombstoned rows (user may soft-delete then
        // hard-delete). Soft delete uses the ordinary resolve (rejects tombstones).
        let sub = if hard {
            if let Some(id) = &resolve.id {
                crate::resolver::resolve_by_id_include_deleted(&db, id)?
            } else {
                // name path: hard delete must reach tombstoned rows too
                // (soft-delete-then-hard-delete-by-name is a valid flow).
                let name = resolve.name.as_ref().expect("checked by resolve() below");
                let cwd = resolve.cwd.as_deref().ok_or_else(|| {
                    SubagentError::InvalidArgument(
                        "cwd is required when name is provided".to_string(),
                    )
                })?;
                crate::resolver::lookup_by_name_include_deleted(&db, name, cwd)?
            }
        } else {
            self.resolve(&db, &resolve)?
        };
        if hard {
            // Application-layer cascade (spec §3.5: no DB-level CASCADE).
            // subagent_hard_delete removes usage, tasks, bindings, memory, events, then the row.
            db.subagent_hard_delete(&sub.id)
                .map_err(SubagentError::Store)?;
            warn!(event_code = "subagent.delete.hard", subagent_id = %sub.id, "hard-deleted subagent");
        } else {
            self.set_logical_status(&db, &sub.id, SubagentStatus::Deleted)?;
            info!(event_code = "subagent.delete.soft", subagent_id = %sub.id, "soft-deleted subagent");
        }
        Ok(sub.id)
    }

    // --- helpers ------------------------------------------------------------

    /// Resolve a single subagent by UUID (`id`) or by name + cwd.
    /// Lookup-only — does NOT create (read/delete ops must not mutate identity).
    /// Exactly one of `id` or `name` must be provided (the control contract),
    /// and when `name` is provided `cwd` is required — the service does NOT
    /// fall back to `"."` for direct RPC clients.
    fn resolve(&self, db: &busytok_store::Database, p: &ResolveParams) -> Result<LogicalSubagent> {
        match (&p.id, &p.name) {
            (Some(_), Some(_)) => {
                return Err(SubagentError::InvalidArgument(
                    "id and name are mutually exclusive".to_string(),
                ));
            }
            (None, None) => {
                return Err(SubagentError::InvalidArgument(
                    "either id or name must be provided".to_string(),
                ));
            }
            _ => {}
        }
        if let Some(id) = &p.id {
            return resolve_by_id(db, id);
        }
        let name = p.name.as_ref().expect("checked above");
        let cwd = p.cwd.as_deref().ok_or_else(|| {
            SubagentError::InvalidArgument("cwd is required when name is provided".to_string())
        })?;
        crate::resolver::lookup_by_name(db, name, cwd)
    }

    /// Whether `profile` is a recognized profile name (built-in or configured).
    fn profile_known(&self, profile: &str) -> bool {
        matches!(
            profile,
            "pi/search-cheap" | "pi/review-cheap" | "pi/plan-cheap"
        ) || self.settings.profiles.contains_key(profile)
    }

    fn set_logical_status(
        &self,
        db: &busytok_store::Database,
        id: &str,
        status: SubagentStatus,
    ) -> Result<()> {
        let mut row = db
            .subagent_get_logical(id)
            .map_err(SubagentError::Store)?
            .ok_or_else(|| SubagentError::NotFound(id.to_string()))?;
        row.status = status.as_str().to_string();
        row.updated_at_ms = busytok_domain::now_ms();
        row.last_active_at_ms = Some(row.updated_at_ms);
        db.subagent_upsert_logical(&row)
            .map_err(SubagentError::Store)?;
        Ok(())
    }
}

fn task_row_to_summary(r: SubagentTaskRow) -> SubagentTaskSummary {
    SubagentTaskSummary {
        id: r.id,
        subagent_id: r.subagent_id,
        profile: r.profile,
        status: r.status.parse().unwrap_or_else(|s| {
            warn!(
                event_code = "subagent.task.parse_status_failed",
                raw_status = %s,
                "failed to parse task status, falling back to Queued"
            );
            TaskStatus::Queued
        }),
        prompt: r.prompt,
        result_summary: r.result_summary,
        error: r.error,
        created_at_ms: r.created_at_ms,
        completed_at_ms: r.completed_at_ms,
    }
}

/// Best-effort classification of a `SubagentError` (from the executor's Err
/// path) into a `TaskErrorKind` for persistence on the task row. Mirrors the
/// executor's `classify_sidecar_error` but operates on the already-converted
/// `SubagentError`. The `SidecarRpc` variant carries a structured `code`
/// field (preserved from `SidecarError::Application(code, ...)` at
/// conversion time), so we match on the numeric code directly — no string
/// parsing. This keeps the manager's classification in sync with the
/// executor's structured `classify_sidecar_error` automatically: both
/// reference the same protocol constants.
fn subagent_error_to_task_error_kind(e: &SubagentError) -> Option<TaskErrorKind> {
    use crate::sidecar::protocol::{AUTH_FAILURE, NETWORK_ERROR, RATE_LIMIT, TASK_TIMEOUT};
    match e {
        SubagentError::SidecarTimeout(_) | SubagentError::TaskTimeout => {
            Some(TaskErrorKind::Timeout)
        }
        SubagentError::SidecarCrashed(_) => Some(TaskErrorKind::Crash),
        // Bug 2 fix: SidecarIo (e.g. Broken pipe) is a network-class failure.
        SubagentError::SidecarIo(_) => Some(TaskErrorKind::Network),
        // Bug 2 fix: SidecarSpawn failure is a crash-class failure.
        SubagentError::SidecarSpawn(_) => Some(TaskErrorKind::Crash),
        // HotSessionLimit is a capacity contention failure. Classified as
        // its own variant so callers can surface "capacity limit reached"
        // instead of the generic "unknown".
        SubagentError::HotSessionLimit { .. } => Some(TaskErrorKind::HotSessionLimit),
        // State divergence (sidecar/DB out of sync) — not retryable as a
        // capacity condition. Classify as Unknown so error_kind is set
        // for observability; the `error` code distinguishes it from
        // HotSessionLimit for callers that need to handle it differently.
        SubagentError::HotSessionStateDivergence(_) => Some(TaskErrorKind::Unknown),
        SubagentError::SidecarRpc { code, .. } => match *code {
            Some(AUTH_FAILURE) => Some(TaskErrorKind::Auth),
            Some(RATE_LIMIT) => Some(TaskErrorKind::RateLimit),
            Some(NETWORK_ERROR) => Some(TaskErrorKind::Network),
            Some(TASK_TIMEOUT) => Some(TaskErrorKind::Timeout),
            _ => Some(TaskErrorKind::Unknown),
        },
        // Store: real DB failures — no retry classification (caller surfaces
        // as "database error"). Validation/NotFound/etc.: config/lookup errors,
        // not task-failure kinds. None = no error_kind persisted.
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sidecar::protocol::{AUTH_FAILURE, NETWORK_ERROR, RATE_LIMIT, TASK_TIMEOUT};
    use busytok_store::SubagentTaskRow;

    /// `subagent_error_to_task_error_kind` maps each sidecar failure variant
    /// to the right `TaskErrorKind` for DB persistence (Task 5). Covers every
    /// match arm so the classifier cannot silently regress.
    #[test]
    fn classifies_sidecar_errors_to_task_error_kind() {
        // Timeout variants.
        assert_eq!(
            subagent_error_to_task_error_kind(&SubagentError::TaskTimeout),
            Some(TaskErrorKind::Timeout)
        );
        assert_eq!(
            subagent_error_to_task_error_kind(&SubagentError::SidecarTimeout("10s".into())),
            Some(TaskErrorKind::Timeout)
        );
        // Crash.
        assert_eq!(
            subagent_error_to_task_error_kind(&SubagentError::SidecarCrashed("sigkill".into())),
            Some(TaskErrorKind::Crash)
        );
        // SidecarRpc with structured codes.
        assert_eq!(
            subagent_error_to_task_error_kind(&SubagentError::SidecarRpc {
                message: "401".into(),
                code: Some(AUTH_FAILURE)
            }),
            Some(TaskErrorKind::Auth)
        );
        assert_eq!(
            subagent_error_to_task_error_kind(&SubagentError::SidecarRpc {
                message: "429".into(),
                code: Some(RATE_LIMIT)
            }),
            Some(TaskErrorKind::RateLimit)
        );
        assert_eq!(
            subagent_error_to_task_error_kind(&SubagentError::SidecarRpc {
                message: "net".into(),
                code: Some(NETWORK_ERROR)
            }),
            Some(TaskErrorKind::Network)
        );
        assert_eq!(
            subagent_error_to_task_error_kind(&SubagentError::SidecarRpc {
                message: "timed out".into(),
                code: Some(TASK_TIMEOUT)
            }),
            Some(TaskErrorKind::Timeout)
        );
        // SidecarRpc with no code → Unknown (Bug 2 fix: was None, now
        // classified so error_kind is always set for sidecar errors).
        assert_eq!(
            subagent_error_to_task_error_kind(&SubagentError::SidecarRpc {
                message: "unknown".into(),
                code: None
            }),
            Some(TaskErrorKind::Unknown)
        );
        // SidecarRpc with an unrecognized code → Unknown.
        assert_eq!(
            subagent_error_to_task_error_kind(&SubagentError::SidecarRpc {
                message: "misc".into(),
                code: Some(-39999)
            }),
            Some(TaskErrorKind::Unknown)
        );
        // HotSessionLimit → HotSessionLimit (dedicated variant, not Unknown).
        assert_eq!(
            subagent_error_to_task_error_kind(&SubagentError::HotSessionLimit {
                candidate: "sess-1".into()
            }),
            Some(TaskErrorKind::HotSessionLimit)
        );
        assert_eq!(
            subagent_error_to_task_error_kind(&SubagentError::HotSessionLimit {
                candidate: "".into()
            }),
            Some(TaskErrorKind::HotSessionLimit)
        );
        // State divergence (sidecar/DB out of sync) → Unknown. error_kind is
        // set so callers can alert; the `error` code distinguishes it from
        // HotSessionLimit for handling decisions.
        assert_eq!(
            subagent_error_to_task_error_kind(&SubagentError::HotSessionStateDivergence(
                "no binding for sess-1".into()
            )),
            Some(TaskErrorKind::Unknown)
        );
        // Bug 2 fix: SidecarIo → Network (was None).
        assert_eq!(
            subagent_error_to_task_error_kind(&SubagentError::SidecarIo(
                "Broken pipe (os error 32)".into()
            )),
            Some(TaskErrorKind::Network)
        );
        // Bug 2 fix: SidecarSpawn → Crash (was None).
        assert_eq!(
            subagent_error_to_task_error_kind(&SubagentError::SidecarSpawn(
                "spawn failed".into()
            )),
            Some(TaskErrorKind::Crash)
        );
        // Non-sidecar errors → None (no error_kind — these are config/lookup
        // errors, not task failures requiring retry classification).
        assert_eq!(
            subagent_error_to_task_error_kind(&SubagentError::Validation("x".into())),
            None
        );
        assert_eq!(
            subagent_error_to_task_error_kind(&SubagentError::NotFound("x".into())),
            None
        );
        assert_eq!(
            subagent_error_to_task_error_kind(&SubagentError::Store(anyhow::anyhow!(
                "db error"
            ))),
            None
        );
    }

    /// `task_row_to_summary` falls back to `TaskStatus::Queued` (with a warn
    /// log) when the DB row's status string is corrupted / unrecognized. This
    /// guards against a half-written migration or a future status enum value
    /// that the running binary doesn't yet know.
    #[test]
    fn task_row_to_summary_falls_back_to_queued_on_bad_status() {
        let _guard = install_tracing_for_unit_test();
        let mut row = SubagentTaskRow::for_test("t-1", "sub-1", "pi/search-cheap", "do something");
        row.status = "not-a-real-status".to_string();
        let summary = task_row_to_summary(row);
        assert_eq!(summary.status, TaskStatus::Queued);
        assert_eq!(summary.id, "t-1");
        assert_eq!(summary.subagent_id, "sub-1");
    }

    /// `task_row_to_summary` parses a known status string without falling back.
    #[test]
    fn task_row_to_summary_parses_known_status() {
        let mut row = SubagentTaskRow::for_test("t-2", "sub-2", "pi/search-cheap", "do something");
        row.status = "completed".to_string();
        row.result_summary = Some("done".into());
        let summary = task_row_to_summary(row);
        assert_eq!(summary.status, TaskStatus::Completed);
        assert_eq!(summary.result_summary.as_deref(), Some("done"));
    }

    // --- P1 fix: u64→i64 timeout truncation unit tests ---

    #[test]
    fn validate_timeout_seconds_rejects_value_exceeding_i64_max() {
        let err = validate_timeout_seconds(9_223_372_036_854_775_808u64).unwrap_err();
        assert!(matches!(err, SubagentError::InvalidArgument(_)));
        assert!(format!("{err}").contains("exceeds i64::MAX"));
    }

    #[test]
    fn validate_timeout_seconds_rejects_value_that_overflows_reaper_arithmetic() {
        // i64::MAX fits in i64 but i64::MAX * 1000 overflows.
        let err = validate_timeout_seconds(i64::MAX as u64).unwrap_err();
        assert!(matches!(err, SubagentError::InvalidArgument(_)));
        assert!(format!("{err}").contains("overflow"));
    }

    #[test]
    fn validate_timeout_seconds_accepts_boundary_value() {
        let boundary = (i64::MAX - REAPER_BUFFER_MS) / 1000;
        assert_eq!(validate_timeout_seconds(boundary as u64).unwrap(), boundary);
    }

    #[test]
    fn validate_timeout_seconds_accepts_normal_values() {
        assert_eq!(validate_timeout_seconds(0).unwrap(), 0);
        assert_eq!(validate_timeout_seconds(300).unwrap(), 300);
        assert_eq!(validate_timeout_seconds(86400).unwrap(), 86400);
    }

    #[test]
    fn cap_timeout_for_reaper_caps_oversized_config() {
        // Value > i64::MAX → capped to MAX_SAFE_TIMEOUT_SECONDS.
        assert_eq!(
            cap_timeout_for_reaper(9_223_372_036_854_775_808u64),
            MAX_SAFE_TIMEOUT_SECONDS
        );
        // i64::MAX (fits i64 but overflows * 1000) → capped.
        assert_eq!(
            cap_timeout_for_reaper(i64::MAX as u64),
            MAX_SAFE_TIMEOUT_SECONDS
        );
    }

    #[test]
    fn cap_timeout_for_reaper_passes_through_normal_config() {
        assert_eq!(cap_timeout_for_reaper(300), 300);
        assert_eq!(cap_timeout_for_reaper(0), 0);
    }

    /// Install a thread-local tracing subscriber so the `warn!` arguments in
    /// the fallback path are evaluated (and counted by line coverage).
    fn install_tracing_for_unit_test() -> tracing::subscriber::DefaultGuard {
        let subscriber = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::TRACE)
            .with_test_writer()
            .finish();
        tracing::subscriber::set_default(subscriber)
    }
}
