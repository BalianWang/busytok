//! The public logical-subagent manager.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use busytok_config::SubagentSettings;
use busytok_store::{
    SubagentMemoryRow, SubagentResourceEventRow, SubagentTaskRow, SubagentUsageRecordRow,
};
use tracing::{error, info, warn};

use crate::context::ContextBuilder;
use crate::error::{Result, SubagentError};
use crate::memory::MemoryUpdater;
use crate::mock_executor::{ExecutorInput, TaskExecutor};
use crate::models::{
    DelegateRequest, DelegateResult, LogicalSubagent, ResolveParams, SubagentStatus,
    SubagentTaskSummary, TaskErrorKind, TaskStatus, TaskUsage,
};
use crate::pressure::PressureGate;
use crate::resolver::{resolve_by_id, resolve_by_name, row_to_model, Resolved};

type SharedDb = Arc<Mutex<busytok_store::Database>>;

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
}

/// Hook invoked by the background dispatcher after a queued task completes
/// successfully. The runtime registers one that calls `bridge_subagent_usage`
/// so queued tasks' usage flows into `usage_events` + rollups.
pub type TaskCompletionHook = Arc<dyn Fn(&DelegateResult, &str) + Send + Sync>;

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

    /// Create-or-continue a subagent and run one task (mock execution in this plan).
    pub async fn delegate(&self, req: DelegateRequest) -> Result<DelegateResult> {
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

        // Spec §3.3: bound fields are conditionally required (create path
        // only). Both must be present or both absent — one without the other
        // is a validation error. Ignored when reusing an existing subagent
        // (name path hit OR subagent_id shortcut).
        let bound_pair = match (&req.bound_provider_id, &req.bound_model_id) {
            (Some(p), Some(m)) => Some((p.clone(), m.clone())),
            (None, None) => None,
            _ => {
                return Err(SubagentError::Validation(
                    "bound_provider_id and bound_model_id must be provided together".into(),
                ));
            }
        };

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

        // 2. insert task row. §8.3 step 2 "queue only" (Round 4 race-free
        //    design) + spec §6.4 per-subagent serialization: check the gate
        //    AND whether this subagent already has a running task BEFORE insert.
        //    - Gate paused OR subagent busy → insert as `"queued"`, return
        //      `Queued` early. The background `TaskDispatcher` (Task 7) picks
        //      it up when the gate clears AND the subagent is free.
        //    - Gate not paused AND subagent free → insert as `"running"`
        //      (with `started_at_ms = now`). The dispatcher only picks
        //      `'queued'` tasks, so it never sees this task.
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
                timeout_seconds: req.timeout_seconds.map(|t| t as i64),
                model_override: req.model_override.clone(),
                error_kind: None,
            })
            .map_err(SubagentError::Store)?;
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
            });
        }

        // 3. Build context + memory snapshot from the store, then execute.
        //    No lock held during execution. Task 7 Step 3: the post-insert
        //    execute logic is delegated to `execute_task()` so the background
        //    dispatcher can reuse it for queued tasks.
        let model = req.model_override.clone();
        // The task row is already inserted as 'running' (Round 4 race-free
        // design). Reconstruct a `SubagentTaskRow` view to pass to
        // `execute_task` — the row is the single source of truth for execution
        // params (Round 3 Finding 1 fix). For the synchronous delegate path
        // we read directly from `req` since the row was just inserted from
        // the same values; the dispatcher path reads the row from the DB.
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
            timeout_seconds: req.timeout_seconds.map(|t| t as i64),
            model_override: req.model_override.clone(),
            error_kind: None,
        };
        let mut result = self.execute_task(&task_row, &subagent).await?;
        // `execute_task` returns the model it actually used (which may differ
        // from `model` when the task row's `model_override` is None and the
        // subagent's `bound_model_id` is used). Preserve the model the caller
        // expected when possible (synchronous path uses `req.model_override`);
        // fall back to the `execute_task` value otherwise.
        if result.model.is_none() {
            result.model = model;
        }
        Ok(result)
    }

    /// Execute a task that is ALREADY "running" (status + started_at_ms set
    /// by the caller). Builds context → executes → persists results.
    /// Called by `delegate()` (synchronous, after inserting as 'running')
    /// and the background dispatcher (after `pick_oldest_queued_task` which
    /// does the atomic CAS flip).
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
        // Spec §4.3 validation chain — fail fast on bound provider/model
        // issues. Reads happen under the DB lock; the resolved values are
        // used below to populate `ExecutorInput`'s provider config + model
        // metadata fields.
        let (resolved_provider, resolved_model) = {
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
                    &effective_model_id,
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
            (provider, model)
        };
        // `model` (the local variable used by the rest of execute_task) is the
        // resolved model id (a String, not Option<String>). The rest of the
        // body uses `effective_model_id` in its place; this shim keeps the
        // existing body compiling without further edits.
        let model: Option<String> = Some(effective_model_id.clone());
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
                model_context_window: resolved_model.context_window.unwrap_or(0),
                model_max_tokens: resolved_model.max_tokens.unwrap_or(0),
                model_display_name: resolved_model.display_name.clone(),
            };
            (
                input,
                memory_row,
                tasks_since_last_compaction,
                profile_cfg.cloned(),
            )
        };
        let out = match self.executor.execute(&input).await.map_err(|e| {
            match e.downcast::<SubagentError>() {
                Ok(se) => se,
                Err(other) => {
                    warn!(event_code = "subagent.delegate.executor_failed", error = %other);
                    SubagentError::Store(other)
                }
            }
        }) {
            Ok(out) => out,
            Err(e) => {
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
                    db.subagent_set_task_status(&task.id, "failed", None, None)
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
            db.subagent_set_task_status(
                &task.id,
                out.status.as_str(),
                Some(out.summary.clone()),
                None,
            )
            .map_err(SubagentError::Store)?;
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

        Ok(DelegateResult {
            task_id: task.id.clone(),
            subagent_id: subagent.id.clone(),
            subagent_name: subagent.name.clone(),
            adapter: self.adapter.clone(),
            adapter_session_id: out.adapter_session_id.clone(),
            session_reused: out.session_reused,
            status: out.status,
            profile: task.profile.clone(),
            model,
            summary: Some(out.summary),
            usage: out.usage.clone(),
        })
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
            loop {
                tokio::select! {
                    _ = ticker.tick() => {},
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
                        let _ = db.subagent_set_task_status(
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
                        "dispatcher executing queued task"
                    );
                    match manager.execute_task(&task, &subagent).await {
                        Ok(result) => {
                            // P1 #2 fix: invoke the post-task completion hook
                            // so the runtime bridges the queued task's usage
                            // into `usage_events` + rollups — the SAME seam as
                            // synchronous `delegate()`. The hook is best-effort:
                            // it logs failures internally and does NOT propagate
                            // errors to the dispatcher. `cwd` is the subagent's
                            // canonical `repo_path` (the cwd `execute_task`
                            // used).
                            let hook = {
                                let guard = manager
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
                            warn!(
                                event_code = "subagent.queue.execute_failed",
                                task_id = %task.id,
                                error = %e,
                                "dispatcher execute_task failed; marking task failed"
                            );
                            let db = manager.db.lock().expect("subagent db lock poisoned");
                            let _ = db.subagent_set_task_status(
                                &task.id,
                                "failed",
                                None,
                                Some(e.to_string()),
                            );
                        }
                    }
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

    /// List subagents, optionally filtered by status / project / include-deleted.
    pub async fn list(
        &self,
        status: Option<SubagentStatus>,
        project: Option<&str>,
        include_deleted: bool,
    ) -> Result<Vec<LogicalSubagent>> {
        let db = self.db.lock().expect("subagent db lock poisoned");
        let rows = db
            .subagent_list_filtered(status.map(|s| s.as_str()), project, include_deleted)
            .map_err(SubagentError::Store)?;
        Ok(rows.iter().map(row_to_model).collect::<Vec<_>>())
    }

    pub async fn show(&self, resolve: ResolveParams) -> Result<LogicalSubagent> {
        let db = self.db.lock().expect("subagent db lock poisoned");
        self.resolve(&db, &resolve)
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
        SubagentError::SidecarRpc { code, .. } => match *code {
            Some(AUTH_FAILURE) => Some(TaskErrorKind::Auth),
            Some(RATE_LIMIT) => Some(TaskErrorKind::RateLimit),
            Some(NETWORK_ERROR) => Some(TaskErrorKind::Network),
            Some(TASK_TIMEOUT) => Some(TaskErrorKind::Timeout),
            _ => None,
        },
        _ => None,
    }
}
