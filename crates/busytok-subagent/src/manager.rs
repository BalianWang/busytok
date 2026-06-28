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
    SubagentTaskSummary, TaskStatus, TaskUsage,
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
        }
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
        let profile_model = self.profile_model(&req.profile);

        // 1. resolve subagent (create if needed). `resolve_by_name` canonicalizes
        //    cwd and validates the name; errors propagate with a reject log.
        let Resolved { subagent, created } = {
            let db = self.db.lock().expect("subagent db lock poisoned");
            if let Some(id) = &req.subagent_id {
                Resolved {
                    subagent: resolve_by_id(&db, id)?,
                    created: false,
                }
            } else {
                match resolve_by_name(
                    &db,
                    &req.subagent_name,
                    &req.cwd,
                    &req.profile,
                    profile_model.as_deref(),
                ) {
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
        //    design): check the gate BEFORE insert and set the row status
        //    directly.
        //    - Gate paused → insert as `"queued"`, return `Queued` early.
        //      The background `TaskDispatcher` (Task 7) picks it up when the
        //      gate clears.
        //    - Gate not paused → insert as `"running"` (with `started_at_ms =
        //      now`). The dispatcher only picks `'queued'` tasks, so it never
        //      sees this task — no race where the dispatcher could pick a
        //      just-inserted queued task before `delegate()` flips it.
        //    `insert_task` already takes `row.status` from the
        //    `SubagentTaskRow` (verified at subagent_queries.rs:331), so no
        //    store change is needed.
        let paused = self
            .pressure_gate
            .as_ref()
            .map(|g| g.is_paused())
            .unwrap_or(false);
        let now = busytok_domain::now_ms();
        let task_id = format!("task_{}", uuid::Uuid::new_v4());
        {
            let db = self.db.lock().expect("subagent db lock poisoned");
            db.subagent_insert_task(&SubagentTaskRow {
                id: task_id.clone(),
                subagent_id: subagent.id.clone(),
                source_harness: req.source_harness.clone(),
                source_session_id: req.source_session_id.clone(),
                intent: req.intent.clone(),
                profile: req.profile.clone(),
                prompt: Some(req.prompt.clone()),
                prompt_artifact_ref: None,
                output_schema_name: None,
                output_schema_version: 1,
                status: if paused {
                    "queued".to_string()
                } else {
                    "running".to_string()
                },
                result_summary: None,
                result_json: None,
                error: None,
                created_at_ms: now,
                started_at_ms: if paused { None } else { Some(now) },
                completed_at_ms: None,
                // Task 7 Round 3 Finding 1 fix: persist execution params so the
                // dispatcher reads them from the row (single source of truth).
                timeout_seconds: req.timeout_seconds.map(|t| t as i64),
                model_override: req.model_override.clone(),
            })
            .map_err(SubagentError::Store)?;
        }

        info!(
            event_code = "subagent.delegate.start",
            subagent_id = %subagent.id,
            created,
            profile = %req.profile,
            "delegating task"
        );

        // If paused, return Queued — the dispatcher handles it when the gate
        // clears. We return immediately without building context or invoking
        // the executor.
        if paused {
            info!(
                event_code = "subagent.delegate.queued",
                subagent_id = %subagent.id,
                task_id = %task_id,
                action = ?self.pressure_gate.as_ref().and_then(|g| g.last_action()),
                "pressure gate paused — task queued, not executed"
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
                model: req.model_override.clone().or(profile_model),
                summary: None,
                usage: TaskUsage::default(),
            });
        }

        // 3. Build context + memory snapshot from the store, then execute.
        //    No lock held during execution. Task 7 Step 3: the post-insert
        //    execute logic is delegated to `execute_task()` so the background
        //    dispatcher can reuse it for queued tasks.
        let model = req.model_override.clone().or(profile_model);
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
            prompt_artifact_ref: None,
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
        };
        let mut result = self.execute_task(&task_row, &subagent).await?;
        // `execute_task` returns the model it actually used (which may differ
        // from `model` when the task row's `model_override` is None and the
        // subagent's `default_model` overrides the profile default). Preserve
        // the model the caller expected when possible (synchronous path uses
        // `req.model_override.or(profile_model)`); fall back to the
        // `execute_task` value otherwise.
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
        let model = task
            .model_override
            .clone()
            .or_else(|| subagent.default_model.clone())
            .or_else(|| self.profile_model(&task.profile));
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
                model: model.clone(),
                prompt,
                timeout_seconds: task.timeout_seconds.map(|t| t as u64),
                tools,
                memory: snapshot,
                context: compact,
                write_access,
            };
            (
                input,
                memory_row,
                tasks_since_last_compaction,
                profile_cfg.cloned(),
            )
        };
        let out = self.executor.execute(&input).await.map_err(|e| {
            match e.downcast::<SubagentError>() {
                Ok(se) => se,
                Err(other) => {
                    warn!(event_code = "subagent.delegate.executor_failed", error = %other);
                    SubagentError::Store(other)
                }
            }
        })?;
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
            let updated_mem = self.memory_updater.update(
                memory_row,
                out.memory_update.clone(),
                &recent_tasks,
                tasks_since_last_compaction,
                &task.id,
                profile_budget,
                &subagent.repo_path,
            );
            info!(
                event_code = "subagent.memory.updated",
                subagent_id = %subagent.id,
                has_hot_summary = updated_mem.hot_summary.is_some(),
                compacted = updated_mem.last_compacted_at_ms.is_some(),
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
                    // canonical `repo_path`, `default_model`, etc.
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
                    if let Err(e) = manager.execute_task(&task, &subagent).await {
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

    fn profile_model(&self, profile: &str) -> Option<String> {
        match profile {
            "pi/search-cheap" => Some(self.settings.models.default_cheap_model.clone()),
            "pi/review-cheap" => Some(self.settings.models.default_review_model.clone()),
            "pi/plan-cheap" => Some(self.settings.models.default_reasoning_model.clone()),
            other => self.settings.profiles.get(other).map(|p| p.model.clone()),
        }
        .filter(|m| !m.is_empty())
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
