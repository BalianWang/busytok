//! WorkerPool — multi-provider supervisor management.
//!
//! Owns one `PiSidecarSupervisor` per active provider. Lazily creates them
//! via `ensure_worker`. Provider credentials are now threaded per-turn via
//! `turn_auto` params (Task 5), not via env injection — `inject_provider_env`
//! was deleted along with the `OPENAI_API_KEY` / `OPENAI_BASE_URL` env
//! injection mechanism.
//!
//! **Two-phase bootstrap (P1c fix):** the responder-factory is NOT passed
//! to `new` — it's set via `set_responder_factory` AFTER the
//! `SidecarTaskExecutor` is constructed (Task 4 does this). The factory is
//! stored in a `OnceLock`; `ensure_worker` panics if unset (fail-fast
//! invariant — bootstrap incomplete).
//!
//! **async kill methods (P1b fix):** `remove_worker_and_kill` and
//! `update_provider_and_kill_old` are self-contained kill + remove. They
//! drop the map lock BEFORE calling `force_kill().await` (don't hold sync
//! `Mutex` across `.await`). `PiSidecarSupervisor` has NO `Drop` fallback,
//! so the kill MUST be explicit and awaited — these methods ensure callers
//! don't forget.
//!
//! **pool.rs:20-24 invariant:** `PiSidecarSupervisor` has no `Drop`
//! fallback. Kill must be explicit and awaited. Sync `workers.remove(&pid)`
//! only removes from map — does NOT terminate the sidecar child process.
//! Do NOT introduce sync `remove_worker(provider_id)` variants.
//!
//! **Shared `PressureGate`:** the same gate is passed to every
//! supervisor's responder (production: one global gate; tests: per-pool
//! gate).
//!
//! **`ensure_worker` is SYNCHRONOUS:** the body is entirely sync (entry
//! lookup + config build + supervisor alloc + responder set + insert;
//! no `.await`). Locking: (1) look up `ProviderRuntimeEntry` OUTSIDE the
//! map lock (clone the entry); (2) acquire map lock, re-check if entry
//! exists (someone else may have created it while we read), if yes →
//! return existing; (3) if no entry → build config + construct supervisor
//! + construct responder via factory + `set_pressure_responder` + insert
//! + return Arc.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock, Weak};

use anyhow::Result;
use tracing::{debug, info, warn};

use busytok_config::SubagentResourcePolicyConfig;

use crate::pressure::{PressureGate, PressureResponder};
use crate::sidecar::{
    PiSidecarSupervisor, SharedDb, SidecarConfig, SidecarError, WorkerSnapshot, SESSION_NOT_FOUND,
};

/// Provider runtime entry — everything WorkerPool needs to spawn a worker.
/// Replaces the old `ProviderLookup` + `CredentialReader` closures.
/// Populated from SQL at construction; updated via
/// `update_provider_and_kill_old` when `provider_changed` fires.
#[derive(Debug, Clone)]
pub struct ProviderRuntimeEntry {
    pub provider_id: String,
    pub api_key: String,
    pub base_url: String,
}

/// Responder factory closure (P1c two-phase init). Takes a
/// `Weak<PiSidecarSupervisor>` (so the responder holds a weak ref,
/// breaking the supervisor → responder → executor → supervisor cycle)
/// and returns an `Arc<PressureResponder>`. The factory is responsible
/// for keeping the strong ref alive (production: `BusytokSupervisor`
/// holds it; tests: a shared holder).
pub type ResponderFactory =
    Arc<dyn Fn(Weak<PiSidecarSupervisor>) -> Arc<PressureResponder> + Send + Sync>;

/// `WorkerPool` manages one `PiSidecarSupervisor` per active provider.
///
/// See the module docs for the P1/P1b/P1c/I1/I2 fixes encoded in this type.
pub struct WorkerPool {
    /// Base sidecar config — cloned per provider (no env override —
    /// credentials flow via `turn_auto` params, Task 5).
    /// Produced by `resolve_base_sidecar_config`.
    base_config: SidecarConfig,
    /// Optional shared DB handle — threaded to each supervisor.
    db: Option<SharedDb>,
    /// Provider runtime entries — keyed by provider_id. Populated from SQL
    /// at construction; updated by supervisor when provider config changes
    /// (`provider_changed` → `update_provider_and_kill_old`).
    providers: Arc<Mutex<HashMap<String, ProviderRuntimeEntry>>>,
    /// Shared pressure gate — passed to every supervisor's responder.
    /// `Some` in production (one global gate); `None` in tests that don't
    /// exercise the gate.
    pressure_gate: Option<Arc<PressureGate>>,
    /// Resource policy for every supervisor (threaded from settings so
    /// `monitor_interval_seconds` / `memory_pressure_free_mb` flow through).
    resource_policy: SubagentResourcePolicyConfig,
    /// Responder factory (P1c two-phase init). Set ONCE via
    /// `set_responder_factory` AFTER `SidecarTaskExecutor` is constructed.
    /// `ensure_worker` reads it via `.get().expect(...)` (fail-fast if
    /// unset). The factory takes a `Weak<PiSidecarSupervisor>` (so the
    /// responder holds a weak ref, breaking the supervisor → responder →
    /// executor → supervisor cycle) and returns an `Arc<PressureResponder>`.
    /// The factory is responsible for keeping the strong ref alive (in
    /// production, Task 4's `BusytokSupervisor` holds it; in tests, a
    /// shared holder keeps it).
    responder_factory: OnceLock<ResponderFactory>,
    /// Per-provider supervisor map. `std::sync::Mutex` (not `tokio::sync`)
    /// because the critical sections are sync (insert / lookup / remove);
    /// async methods (`remove_worker_and_kill`,
    /// `update_provider_and_kill_old`, `shutdown_all`,
    /// `worker_snapshots`) drop the lock BEFORE any `.await`.
    workers: Arc<Mutex<HashMap<String, Arc<PiSidecarSupervisor>>>>,
}

impl WorkerPool {
    /// Construct a new `WorkerPool`.
    ///
    /// `providers` is the initial set of `ProviderRuntimeEntry` values,
    /// keyed by provider_id. Typically populated from SQL at construction
    /// time; updated via `update_provider_and_kill_old` when
    /// `provider_changed` fires.
    /// `pressure_gate` is the shared gate passed to every supervisor —
    /// `Some` in production, `None` in tests that don't exercise the gate.
    ///
    /// The responder-factory is NOT passed here — call
    /// `set_responder_factory` AFTER the `SidecarTaskExecutor` is
    /// constructed (P1c two-phase init). `ensure_worker` will panic
    /// (fail-fast) if the factory is unset when called.
    pub fn new(
        base_config: SidecarConfig,
        db: Option<SharedDb>,
        providers: HashMap<String, ProviderRuntimeEntry>,
        pressure_gate: Option<Arc<PressureGate>>,
        resource_policy: SubagentResourcePolicyConfig,
    ) -> Self {
        Self {
            base_config,
            db,
            providers: Arc::new(Mutex::new(providers)),
            pressure_gate,
            resource_policy,
            responder_factory: OnceLock::new(),
            workers: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Set the responder-factory (P1c two-phase init). Called ONCE during
    /// bootstrap AFTER `SidecarTaskExecutor` is constructed — the factory
    /// needs the executor to build the `PressureResponder`, and the
    /// executor needs the supervisor, which is created lazily by
    /// `ensure_worker`. So the wiring order is:
    ///   1. `WorkerPool::new(...)`
    ///   2. `pool.set_responder_factory(...)` (closure captures executor weak)
    ///   3. `pool.ensure_worker(pid)` (constructs supervisor + responder)
    ///
    /// Calling this more than once is a no-op (the second call is logged
    /// and ignored) — `OnceLock::set` rejects overwrites.
    pub fn set_responder_factory(&self, factory: ResponderFactory) {
        if self.responder_factory.set(factory).is_err() {
            warn!(
                event_code = "subagent.pool.responder_factory_already_set",
                "set_responder_factory called more than once — ignoring"
            );
        }
    }

    /// Lazily create (or return existing) supervisor for `provider_id`.
    ///
    /// Synchronous — body is entirely sync: entry lookup + config build +
    /// supervisor alloc + responder set + insert; no `.await`.
    ///
    /// # Locking
    /// 1. Look up `ProviderRuntimeEntry` in the providers map (clone the
    ///    entry — cheap, no OS I/O).
    /// 2. Acquire workers map lock, re-check if entry exists (someone else
    ///    may have created it while we read); if yes → return existing Arc.
    /// 3. If no entry → build config + construct supervisor + construct
    ///    responder via factory (panics if `set_responder_factory` not yet
    ///    called — P1c fail-fast) + `set_pressure_responder` + insert +
    ///    return Arc.
    ///
    /// # Errors
    /// - `SidecarError::Spawn("unknown provider: ...")` — provider not in
    ///   the `providers` map.
    pub fn ensure_worker(
        &self,
        provider_id: &str,
    ) -> Result<Arc<PiSidecarSupervisor>, SidecarError> {
        // (1) Look up provider runtime entry (clone — no lock held during
        // supervisor construction). Task 5: the entry's api_key + base_url
        // are no longer consumed here (credentials flow via `turn_auto`
        // params), but the lookup is kept so unknown providers fail fast
        // with "unknown provider" before constructing a supervisor.
        let _entry = {
            let providers = self.providers.lock().expect("providers map lock poisoned");
            providers
                .get(provider_id)
                .cloned()
                .ok_or_else(|| SidecarError::Spawn(format!("unknown provider: {provider_id}")))?
        };

        // (2) Acquire workers map lock + re-check (someone else may have
        // created the supervisor while we read the entry). This is the fast
        // path: most calls hit the cached entry and return here.
        {
            let workers = self.workers.lock().expect("workers map lock poisoned");
            if let Some(sup) = workers.get(provider_id) {
                debug!(
                    event_code = "subagent.worker_pool.worker_reused",
                    provider_id = %provider_id,
                    "ensure_worker: returning existing supervisor (fast path)"
                );
                return Ok(Arc::clone(sup));
            }
        }

        // (3) No entry → build config + construct supervisor + responder +
        // insert. Constructed OUTSIDE the workers map lock so the (cheap,
        // sync) allocation doesn't block other callers' fast-path lookups.
        // Task 5: provider credentials flow via `turn_auto` params (no env
        // injection); the base config is cloned as-is.
        let config = self.base_config.clone();

        // Construct supervisor with shared pressure gate.
        let policy = self.resource_policy.clone();
        let sup = PiSidecarSupervisor::with_resource_policy(
            config,
            self.db.clone(),
            policy,
            self.pressure_gate.clone(),
        );

        // Construct responder via factory (P1c fail-fast). The factory
        // is responsible for keeping the strong Arc<PressureResponder>
        // alive (e.g. by storing it in a holder); set_pressure_responder
        // stores a Weak, which stays upgradeable only as long as the
        // factory's strong ref exists.
        let factory = self.responder_factory.get().expect(
            "responder_factory not set — bootstrap incomplete; \
             call set_responder_factory before ensure_worker",
        );
        let responder = factory(Arc::downgrade(&sup));
        sup.set_pressure_responder(responder);

        // Insert under lock; if another caller raced and inserted first,
        // drop ours and return theirs (no duplicate supervisors). This is
        // the slow-path race window between the re-check above and here —
        // rare in practice (concurrent ensure_worker for the same provider
        // is the only trigger).
        let mut workers = self.workers.lock().expect("workers map lock poisoned");
        if let Some(existing) = workers.get(provider_id) {
            // Race lost — another caller inserted first. Drop ours (the
            // supervisor hasn't been started, so no kill is needed), and
            // return theirs.
            debug!(
                event_code = "subagent.worker_pool.worker_race_lost",
                provider_id = %provider_id,
                "ensure_worker: race lost, returning existing supervisor"
            );
            return Ok(Arc::clone(existing));
        }
        workers.insert(provider_id.to_string(), Arc::clone(&sup));
        debug!(
            event_code = "subagent.worker_pool.worker_created",
            provider_id = %provider_id,
            "ensure_worker: created and inserted new supervisor"
        );
        Ok(sup)
    }

    /// Hard-remove + kill a worker (P1b fix). Self-contained: callers
    /// don't need to remember to kill — this method does it. The provider
    /// entry in the `providers` map is LEFT IN PLACE so the next
    /// `ensure_worker` can re-spawn (e.g. after an auth-fail kill — the
    /// credential may have been refreshed; the worker should be re-created
    /// from the existing entry). To also remove the provider entry (for
    /// disabled / deleted providers), call `remove_provider_entry`
    /// separately.
    ///
    /// # Locking (I1 fix)
    /// 1. Acquire workers map lock, `remove` entry → `Option<Arc<...>>`.
    /// 2. DROP the lock.
    /// 3. If `Some(sup)`, `sup.force_kill().await` OUTSIDE the lock
    ///    (force_kill awaits `child.wait()` — must not hold sync mutex
    ///    across `.await`).
    ///
    /// `PiSidecarSupervisor` has NO `Drop` fallback, so the kill MUST be
    /// explicit and awaited — this method ensures callers don't forget.
    /// If the supervisor was never started, `force_kill` is a no-op on
    /// `None` child (safe to call).
    pub async fn remove_worker_and_kill(&self, provider_id: &str) -> Result<()> {
        let sup = {
            let mut workers = self.workers.lock().expect("workers map lock poisoned");
            workers.remove(provider_id)
        };
        if let Some(sup) = sup {
            debug!(
                event_code = "subagent.worker_pool.remove_and_kill",
                provider_id = %provider_id,
                "remove_worker_and_kill: force-killing supervisor"
            );
            sup.force_kill().await;
            debug!(
                event_code = "subagent.worker_pool.remove_and_kill_done",
                provider_id = %provider_id,
                "remove_worker_and_kill: supervisor killed and removed"
            );
        } else {
            debug!(
                event_code = "subagent.worker_pool.remove_and_kill_noop",
                provider_id = %provider_id,
                "remove_worker_and_kill: no worker found (already removed?)"
            );
        }
        Ok(())
    }

    /// Remove ONLY the provider entry from the `providers` map (no kill).
    /// Synchronous — no `.await`. Used by `provider_deleted` and the
    /// disabled/missing branch of `provider_changed` to drop the provider
    /// entry so subsequent `ensure_worker` calls fail with "unknown
    /// provider" (correct for disabled / deleted providers). The worker
    /// itself is killed separately via `remove_worker_and_kill`.
    ///
    /// NOTE: the auth-fail kill path in the executor calls
    /// `remove_worker_and_kill` ONLY (not this method) — the provider
    /// entry must stay so the next `ensure_worker` can re-spawn after a
    /// bad-credential kill (the key may have been refreshed).
    pub fn remove_provider_entry(&self, provider_id: &str) {
        let mut providers = self.providers.lock().expect("providers map lock poisoned");
        providers.remove(provider_id);
    }

    /// Update or insert a provider's runtime entry, then force-kill the
    /// existing worker (if any) so the next delegate re-spawns with the
    /// new credentials/base_url. Called by supervisor on provider_changed.
    ///
    /// MUST be async: `force_kill().await` waits for `child.wait()` and
    /// must not be skipped (no `Drop` fallback in `PiSidecarSupervisor`).
    /// Drops the map lock BEFORE `.await` (don't hold sync `Mutex` across
    /// await — see pool.rs:20-24 invariant).
    pub async fn update_provider_and_kill_old(&self, entry: ProviderRuntimeEntry) -> Result<()> {
        let pid = entry.provider_id.clone();
        {
            let mut providers = self.providers.lock().expect("providers map lock poisoned");
            providers.insert(pid.clone(), entry);
        }
        // Take the old worker out of the map, then force-kill it OUTSIDE
        // the lock (force_kill awaits child.wait()).
        let old = {
            let mut workers = self.workers.lock().expect("workers map lock poisoned");
            workers.remove(&pid)
        };
        if let Some(sup) = old {
            info!(
                event_code = "subagent.worker_pool.update_provider_kill",
                provider_id = %pid,
                "update_provider_and_kill_old: force-killing old supervisor"
            );
            sup.force_kill().await;
            info!(
                event_code = "subagent.worker_pool.update_provider_killed",
                provider_id = %pid,
                "update_provider_and_kill_old: old supervisor killed"
            );
        }
        Ok(())
    }

    /// Read-only snapshots of all workers for `runtime_status` aggregation.
    ///
    /// Async because the underlying `PiSidecarSupervisor::worker_snapshot`
    /// is async (acquires the supervisor's `tokio::sync::Mutex`).
    /// Lock-ordering: collect `(provider_id, supervisor)` pairs under the
    /// map lock, DROP the lock, then call `worker_snapshot().await` on
    /// each OUTSIDE the map lock (never hold a sync mutex across
    /// `.await`).
    pub async fn worker_snapshots(&self) -> Vec<(String, WorkerSnapshot)> {
        let pairs: Vec<(String, Arc<PiSidecarSupervisor>)> = {
            let workers = self.workers.lock().expect("workers map lock poisoned");
            workers
                .iter()
                .map(|(pid, sup)| (pid.clone(), Arc::clone(sup)))
                .collect()
        };
        let mut out = Vec::with_capacity(pairs.len());
        for (pid, sup) in pairs {
            if let Some(snap) = sup.worker_snapshot().await {
                out.push((pid, snap));
            }
        }
        out
    }

    /// Gracefully shut down all workers. Same lock-ordering as
    /// `remove_worker_and_kill`: collect all entries under lock, drop
    /// lock, then call `shutdown().await` on each supervisor outside the
    /// lock. Used by `BusytokSupervisor::shutdown_sidecar` (service exit)
    /// and `rebuild_sidecar_runtime` (mid-flight config change) — both
    /// need the FULL pool drained, not just the single "first enabled
    /// provider" supervisor, so no orphaned Node subprocesses survive
    /// config flips or service exit. Best-effort: per-worker failures
    /// are logged but don't abort the loop.
    pub async fn shutdown_all(&self) {
        let supervisors: Vec<Arc<PiSidecarSupervisor>> = {
            let mut workers = self.workers.lock().expect("workers map lock poisoned");
            workers.drain().map(|(_, v)| v).collect()
        };
        let count = supervisors.len();
        debug!(
            event_code = "subagent.worker_pool.shutdown_all_start",
            worker_count = count,
            "shutdown_all: gracefully shutting down {} supervisor(s)",
            count
        );
        for sup in supervisors {
            if let Err(e) = sup.shutdown().await {
                warn!(
                    event_code = "subagent.worker_pool.shutdown_one_failed",
                    error = %e,
                    "shutdown_all: one supervisor graceful-shutdown failed (continuing)"
                );
            }
        }
        debug!(
            event_code = "subagent.worker_pool.shutdown_all_done",
            worker_count = count,
            "shutdown_all: all supervisors gracefully shut down"
        );
    }

    /// Iterate over all supervisors (sync). For `evict_lru` iteration
    /// across all providers (Task 3, I5 fix). The closure receives the
    /// provider_id and a strong `Arc<PiSidecarSupervisor>` ref.
    pub fn for_each_supervisor(&self, mut f: impl FnMut(&str, &Arc<PiSidecarSupervisor>)) {
        let workers = self.workers.lock().expect("workers map lock poisoned");
        for (pid, sup) in workers.iter() {
            f(pid, sup);
        }
    }

    /// Get an existing worker WITHOUT creating one. Returns `None` if no
    /// supervisor exists for `provider_id` (used by best-effort cancel —
    /// we don't want to spawn a sidecar just to cancel a session).
    pub fn get_worker(&self, provider_id: &str) -> Option<Arc<PiSidecarSupervisor>> {
        let workers = self.workers.lock().expect("workers map lock poisoned");
        workers.get(provider_id).cloned()
    }

    /// Look up which provider's supervisor owns a given adapter session
    /// (C7 fix: `evict_session` needs this to route `prepare_hibernate` /
    /// `close` RPCs to the correct supervisor in a multi-provider pool).
    ///
    /// Returns `None` if no supervisor owns the session (binding belongs to
    /// a removed provider, or no binding exists for the session).
    ///
    /// **Routing strategy:** the binding schema (`subagent_harness_bindings`)
    /// stores `harness` but NOT `provider_id`. So we iterate the pool's
    /// supervisors and query `find_binding_by_session` (no `is_hot` filter)
    /// for each supervisor's `harness_name`. The first match wins. Since all
    /// current providers use `harness_name = "pi"`, the first supervisor
    /// with a matching harness is returned. This is O(n) in the number of
    /// providers (small — typically 1-3) and avoids a schema migration to
    /// add `provider_id` to the binding table.
    ///
    /// **Why not `find_hot_binding_by_session`?** A concurrent evictor may
    /// have already flipped the binding to `is_hot=0` (Bug 1 race). The
    /// supervisor that owns the session is the same regardless of the
    /// binding's hot/cold state — using the non-hot query lets us route
    /// `prepare_hibernate` / `close` RPCs correctly even after the flip.
    /// If the session was already closed, `prepare_hibernate` returns
    /// `SESSION_NOT_FOUND` and `evict_session` returns `AlreadyEvicted`.
    pub fn supervisor_for_session(
        &self,
        adapter_session_id: &str,
    ) -> Option<(String, Arc<PiSidecarSupervisor>)> {
        // Collect (provider_id, supervisor, harness_name) under the map
        // lock, then release before querying the DB (DB lock is a
        // `std::sync::Mutex` — never hold both locks simultaneously to
        // avoid lock-ordering issues).
        let candidates: Vec<(String, Arc<PiSidecarSupervisor>, String)> = {
            let workers = self.workers.lock().expect("workers map lock poisoned");
            workers
                .iter()
                .map(|(pid, sup)| {
                    (
                        pid.clone(),
                        Arc::clone(sup),
                        sup.config().harness_name.clone(),
                    )
                })
                .collect()
        };
        let Some(db) = &self.db else {
            // No DB — can't query bindings. Return the first supervisor
            // (single-provider fallback for no-DB test paths).
            return candidates
                .into_iter()
                .next()
                .map(|(pid, sup, _)| (pid, sup));
        };
        let db = db.lock().expect("db lock poisoned");
        for (pid, sup, harness) in &candidates {
            if let Ok(Some(_binding)) =
                db.subagent_find_binding_by_session(adapter_session_id, harness)
            {
                return Some((pid.clone(), Arc::clone(sup)));
            }
        }
        None
    }

    /// Best-effort purge of a ghost session from all sidecar pools.
    ///
    /// Called by `evict_session` when `supervisor_for_session` returns `None`
    /// (no DB binding for the candidate). The session exists in one
    /// sidecar's in-memory pool but has no DB binding — a "ghost" left
    /// behind by a failed/cancelled/timed-out `turn_auto` that created a
    /// session but never committed a binding.
    ///
    /// Iterates all supervisors and calls `session.close` on each. The
    /// sidecar that holds the session returns `{ ok: true }`; others
    /// return `SESSION_NOT_FOUND` (-32001). Returns `true` if any
    /// supervisor's close succeeded, `false` otherwise.
    ///
    /// This is defense-in-depth: the sidecar's `turn_auto` ghost cleanup
    /// (closing newly-created sessions on failure) is the primary fix.
    /// This method handles residual ghosts from pre-fix code, the
    /// cancel-while-succeeding edge case, or sidecar/DB desync.
    pub async fn purge_session(&self, adapter_session_id: &str) -> bool {
        let supervisors: Vec<Arc<PiSidecarSupervisor>> = {
            let workers = self.workers.lock().expect("workers map lock poisoned");
            workers.values().map(Arc::clone).collect()
        };
        for sup in &supervisors {
            let handle = match sup.ensure_started().await {
                Ok(h) => h,
                Err(e) => {
                    warn!(
                        event_code = "subagent.session.purge_supervisor_unavailable",
                        adapter_session_id = %adapter_session_id,
                        error = %e,
                        "could not ensure supervisor for purge — trying next"
                    );
                    continue;
                }
            };
            match handle.close(adapter_session_id).await {
                Ok(_) => {
                    info!(
                        event_code = "subagent.session.ghost_purged",
                        adapter_session_id = %adapter_session_id,
                        "ghost session purged from sidecar pool"
                    );
                    return true;
                }
                Err(SidecarError::Application(code, _msg, _data)) if code == SESSION_NOT_FOUND => {
                    // Session not in this supervisor's pool — try next.
                    continue;
                }
                Err(e) => {
                    warn!(
                        event_code = "subagent.session.purge_close_failed",
                        adapter_session_id = %adapter_session_id,
                        error = %e,
                        "close RPC failed during purge — trying next supervisor"
                    );
                    continue;
                }
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    // Task 5: `inject_provider_env` was deleted — provider credentials now
    // flow via `turn_auto` params. The `OPENAI_API_KEY` / `OPENAI_BASE_URL`
    // env-injection test was removed with it.
}
