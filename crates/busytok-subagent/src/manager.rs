//! The public logical-subagent manager.

use std::sync::{Arc, Mutex};

use busytok_config::SubagentSettings;
use busytok_store::{
    SubagentMemoryRow, SubagentResourceEventRow, SubagentTaskRow, SubagentUsageRecordRow,
};
use tracing::{info, warn};

use crate::error::{Result, SubagentError};
use crate::mock_executor::run_mock;
use crate::models::{
    DelegateRequest, DelegateResult, LogicalSubagent, ResolveParams, SubagentStatus,
    SubagentTaskSummary, TaskStatus,
};
use crate::resolver::{resolve_by_id, resolve_by_name, row_to_model, Resolved};

type SharedDb = Arc<Mutex<busytok_store::Database>>;

pub struct SubagentManager {
    db: SharedDb,
    settings: SubagentSettings,
    adapter: String,
}

impl SubagentManager {
    pub fn new(db: SharedDb, settings: SubagentSettings, adapter: &str) -> Self {
        Self {
            db,
            settings,
            adapter: adapter.to_string(),
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

        // 2. insert task row (queued)
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
                status: "queued".to_string(),
                result_summary: None,
                result_json: None,
                error: None,
                created_at_ms: busytok_domain::now_ms(),
                started_at_ms: None,
                completed_at_ms: None,
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

        // 3. mock-execute (Plan 2: sidecar turn). No lock held during execution.
        let model = req.model_override.clone().or(profile_model);
        let started = busytok_domain::now_ms();
        let out = run_mock(&req.prompt, model.as_deref());
        let duration_ms = busytok_domain::now_ms().saturating_sub(started);

        // 4. persist results: task status, usage, memory (hot_summary), status.
        //    Writing hot_summary satisfies the `warm` invariant (recoverable
        //    memory exists). Plan 1 records NO hot binding (no real session),
        //    so status is Warm, not Hot — consistent with spec §3.3.
        {
            let db = self.db.lock().expect("subagent db lock poisoned");
            db.subagent_set_task_status(
                &task_id,
                out.status.as_str(),
                Some(out.summary.clone()),
                None,
            )
            .map_err(SubagentError::Store)?;
            db.subagent_insert_usage_record(&SubagentUsageRecordRow {
                id: format!("usage_{task_id}"),
                task_id: task_id.clone(),
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

            // memory: write hot_summary so hibernate/restore recovers context.
            self.write_hot_summary(&db, &subagent.id, &out.summary)?;
            self.set_logical_status(&db, &subagent.id, SubagentStatus::Warm)?;
        }

        Ok(DelegateResult {
            task_id,
            subagent_id: subagent.id.clone(),
            subagent_name: subagent.name.clone(),
            adapter: self.adapter.clone(),
            adapter_session_id: None,
            session_reused: !created,
            status: out.status,
            profile: req.profile,
            model,
            summary: Some(out.summary),
            usage: out.usage.clone(),
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
        let binding = db
            .subagent_hot_binding(&sub.id, &self.adapter)
            .map_err(SubagentError::Store)?;
        if let Some(mut b) = binding {
            let now = busytok_domain::now_ms();
            b.is_hot = 0;
            b.status = "closed".to_string();
            b.closed_at_ms = Some(now);
            db.subagent_upsert_binding(&b)
                .map_err(SubagentError::Store)?;
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
        }
        // status follows the invariant: memory exists → warm, else cold
        let new_status = match db
            .subagent_get_memory(&sub.id)
            .map_err(SubagentError::Store)?
            .and_then(|m| m.hot_summary)
        {
            Some(_) => SubagentStatus::Warm,
            None => SubagentStatus::Cold,
        };
        self.set_logical_status(&db, &sub.id, new_status)?;
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
                // name path: tombstones are filtered by lookup_by_name, so a
                // soft-deleted-by-name subagent is NotFound here — acceptable.
                self.resolve(&db, &resolve)?
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
    /// Exactly one of `id` or `name` must be provided (the control contract).
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
        let cwd = p.cwd.as_deref().unwrap_or(".");
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

    /// Persist the most recent task summary as the recoverable `hot_summary`.
    fn write_hot_summary(
        &self,
        db: &busytok_store::Database,
        subagent_id: &str,
        summary: &str,
    ) -> Result<()> {
        let mut mem = db
            .subagent_get_memory(subagent_id)
            .map_err(SubagentError::Store)?
            .unwrap_or_else(|| SubagentMemoryRow::new_empty(subagent_id));
        mem.hot_summary = Some(summary.to_string());
        mem.updated_at_ms = busytok_domain::now_ms();
        db.subagent_upsert_memory(&mem)
            .map_err(SubagentError::Store)?;
        Ok(())
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
