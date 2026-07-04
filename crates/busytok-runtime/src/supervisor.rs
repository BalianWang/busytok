//! BusytokSupervisor: top-level service lifecycle manager.
//!
//! Holds the Database, AppEventBus, PriceCatalog, and adapters.
//! Implements the `RuntimeControl` trait from busytok-control so the
//! control server can dispatch RPC calls to it.
//!
//! Since `Database` (wrapping `rusqlite::Connection`) is not `Send + Sync`,
//! and `AgentLogAdapter` trait objects are not `Send + Sync`, both are
//! wrapped in `Mutex` to satisfy the `RuntimeControl: Send + Sync` bound.

use std::collections::BTreeMap;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use anyhow::{Context, Result};
use async_trait::async_trait;
use rusqlite::Connection;
use tracing::{debug, error, info, warn};

use busytok_config::{BusytokPaths, BusytokSettings};
use busytok_control::dispatch::{MethodDispatchError, RuntimeControl};
use busytok_domain::{now_ms, ReportingTimezone};
use busytok_events::AppEventBus;
use busytok_protocol::dto::*;
use busytok_store::Database;

use crate::queue::ScanStats;
use crate::range;
use crate::scan::{scan_once, scan_once_via_writer};
use crate::status::ServiceStatusSnapshot;
use crate::ui_models;
use crate::writer::{self, WriterHandle};

/// Type alias for boxed adapter with Send + Sync bounds.
type BoxedAdapter = Box<dyn busytok_adapters::AgentLogAdapter + Send + Sync>;

/// Output of `construct_sidecar` — the executor + 6 associated fields.
/// Consumed by `build_sidecar_runtime` which constructs the `SubagentManager`
/// from the executor + pressure_gate and returns a complete `SidecarRuntime`.
struct SidecarComponents {
    executor: Arc<dyn busytok_subagent::mock_executor::TaskExecutor>,
    sidecar_supervisor: Option<Arc<busytok_subagent::sidecar::PiSidecarSupervisor>>,
    sidecar_init_error: Option<String>,
    pressure_gate: Option<Arc<busytok_subagent::PressureGate>>,
    sidecar_executor: Option<Arc<busytok_subagent::sidecar::SidecarTaskExecutor>>,
    pressure_responder: Option<Arc<busytok_subagent::PressureResponder>>,
    worker_pool: Option<Arc<busytok_subagent::sidecar::WorkerPool>>,
}

/// Swappable sidecar runtime bundle — all 7 fields that `construct_sidecar`
/// produces. Wrapped in `RwLock<SidecarRuntime>` on `BusytokSupervisor` so
/// `pi_sidecar_locator_update` can rebuild the entire sidecar subsystem
/// mid-flight (spec §472-475: fresh-install closed loop must work in the
/// CURRENT session, no service restart). Readers acquire a short read lock
/// and clone out the `Arc` they need; the write lock is only held for the
/// pointer swap during rebuild (microseconds).
struct SidecarRuntime {
    /// Logical-subagent manager (subagent.* RPC handlers). Owns the
    /// `TaskExecutor` — swapping the executor requires rebuilding the
    /// manager, so the whole runtime is rebuilt as a unit.
    subagent_manager: Arc<busytok_subagent::SubagentManager>,
    /// Pi sidecar supervisor (present when `pi_sidecar.enabled` AND its
    /// config resolved AND at least one provider is configured).
    sidecar_supervisor: Option<Arc<busytok_subagent::sidecar::PiSidecarSupervisor>>,
    /// Multi-provider worker pool. `Some` when sidecar enabled + config OK.
    worker_pool: Option<Arc<busytok_subagent::sidecar::WorkerPool>>,
    /// Pressure gate shared between supervisor (writer) and manager (reader).
    pressure_gate: Option<Arc<busytok_subagent::PressureGate>>,
    /// Concrete `SidecarTaskExecutor` Arc — strong owner.
    sidecar_executor: Option<Arc<busytok_subagent::sidecar::SidecarTaskExecutor>>,
    /// Escalation chain driver. Strong-owned here; supervisor holds `Weak`.
    pressure_responder: Option<Arc<busytok_subagent::PressureResponder>>,
    /// Error message when sidecar config could not be resolved despite
    /// `enabled = true`. `None` when OK or when disabled.
    sidecar_init_error: Option<String>,
}

/// Top-level service supervisor that orchestrates the Busytok runtime.
///
/// Owns the database, event bus, and adapter list. Implements `RuntimeControl`
/// so the control server can dispatch RPC calls to it.
pub struct BusytokSupervisor {
    /// Database wrapped in Arc<Mutex> for thread safety and sharing with tailer.
    db: Arc<Mutex<Database>>,
    /// Event bus (already Send + Sync).
    event_bus: Arc<AppEventBus>,
    /// Source discovery orchestrator.
    source_registry: crate::source_registry::SourceRegistry,
    /// Generation and readiness state manager. Wrapped in `Arc` so the
    /// background dispatcher's task-completion hook (P1 #2 fix) can share
    /// the same `active_generation_id` cache as `write_subagent_usage_event`.
    generation_manager: Arc<crate::generation_manager::GenerationManager>,
    /// Adapter list wrapped in Mutex for thread safety.
    adapters: Mutex<Vec<BoxedAdapter>>,
    /// Resolved filesystem paths.
    paths: BusytokPaths,
    /// Current scan statistics (updated after each scan).
    last_scan_stats: Mutex<Option<ScanStats>>,
    /// Persisted settings (TOML-backed).
    settings: Arc<Mutex<BusytokSettings>>,
    /// In-memory service status snapshot for the `shell.status` fast path.
    status: Arc<tokio::sync::RwLock<ServiceStatusSnapshot>>,
    /// Handle for sending commands to the bounded writer actor.
    writer_handle: WriterHandle,
    /// Bounded read-only service for overview/activity read paths.
    read_service: crate::read_service::ReadService,
    /// Swappable sidecar runtime (7-field bundle). Wrapped in `RwLock` so
    /// `pi_sidecar_locator_update` can rebuild the entire sidecar subsystem
    /// when `enabled` flips to `true` mid-flight (spec §472-475 fresh-install
    /// closed loop). Readers hold the read lock for microseconds (clone an
    /// `Arc` out); the write lock is only held for the pointer swap.
    sidecar_runtime: std::sync::RwLock<SidecarRuntime>,
    /// Serializes `rebuild_sidecar_runtime` calls so concurrent
    /// `pi_sidecar_locator_update` RPCs can't leak dispatcher tasks (one
    /// call takes the dispatcher handle, the other gets `None`, and the
    /// second call's new dispatcher overwrites the first's — leaking it).
    /// `tokio::sync::Mutex` (not `std::sync::Mutex`) because the rebuild
    /// holds the guard across `.await` points (drain + shutdown).
    rebuild_lock: tokio::sync::Mutex<()>,
    /// JoinHandle for the writer actor's background task (None when no
    /// Tokio runtime was active at construction time, e.g. sync tests).
    _writer_join: Mutex<Option<tokio::task::JoinHandle<()>>>,
    /// JoinHandle for the catalog reload background task (None when no
    /// Tokio runtime was active at construction time).
    _catalog_reload_join: Mutex<Option<tokio::task::JoinHandle<()>>>,
    /// §8.3 step 2 "queue only" background task dispatcher (Task 7).
    /// Polls `subagent_tasks` for queued rows and executes them when the
    /// pressure gate is not paused. `None` when no Tokio runtime was
    /// active at construction time. Wrapped in `Mutex<Option<...>>` so
    /// `shutdown_writer(&self)` (which takes `&self`, not `&mut self`)
    /// can `.take()` the handle.
    task_dispatcher: Mutex<Option<tokio::task::JoinHandle<()>>>,
    /// Shutdown sender for `task_dispatcher` (Task 7 Finding 3 fix).
    /// `JoinHandle` drop = detach (NOT abort), so explicit shutdown
    /// signaling via `tokio::sync::watch` is required. `None` when no
    /// Tokio runtime was active at construction time.
    dispatcher_shutdown: Mutex<Option<tokio::sync::watch::Sender<bool>>>,
}

enum DatabaseHandle<'a> {
    Shared(std::sync::MutexGuard<'a, Database>),
    Detached(Database),
}

impl Deref for DatabaseHandle<'_> {
    type Target = Database;

    fn deref(&self) -> &Self::Target {
        match self {
            Self::Shared(db) => db,
            Self::Detached(db) => db,
        }
    }
}

impl BusytokSupervisor {
    const ACTIVE_SCAN_GRACE_MS: i64 = 10 * 60 * 1000;
    const LIVE_BUCKET_MS: i64 = 2_000;

    /// Create a new supervisor with the given database and configuration.
    pub fn new(db: Database, paths: BusytokPaths) -> Self {
        let adapters: Vec<BoxedAdapter> = vec![
            Box::new(busytok_adapters::ClaudeCodeAdapter),
            Box::new(busytok_adapters::CodexAdapter),
        ];
        Self::build(db, paths, adapters)
    }

    /// Create a supervisor with custom adapters (for testing or extensions).
    pub fn with_adapters(db: Database, paths: BusytokPaths, adapters: Vec<BoxedAdapter>) -> Self {
        Self::build(db, paths, adapters)
    }

    /// Create a supervisor with custom adapters and pre-loaded settings.
    /// Exposed so integration tests can exercise the production config-
    /// resolution path (including sidecar config failure → FailingTaskExecutor).
    #[doc(hidden)]
    pub fn with_adapters_and_settings(
        db: Database,
        paths: BusytokPaths,
        adapters: Vec<BoxedAdapter>,
        settings: BusytokSettings,
    ) -> Self {
        Self::build_with_settings(db, paths, adapters, settings)
    }

    /// Test-only accessor for the in-memory pi_sidecar locator state.
    /// Used by integration tests to verify `pi_sidecar_locator_update`
    /// mutated the shared Arc<Mutex<BusytokSettings>> (not just the file).
    /// The `settings` field is private; `settings_snapshot` RPC returns a
    /// DTO that omits `pi_sidecar.runtime_dir`, so this is the only way
    /// for tests to observe the in-memory locator state directly.
    #[doc(hidden)]
    pub fn pi_sidecar_state(&self) -> (Option<String>, bool) {
        let s = self.settings.lock().unwrap();
        (
            s.subagent.pi_sidecar.runtime_dir.clone(),
            s.subagent.pi_sidecar.enabled,
        )
    }

    /// Construct a supervisor with a pre-resolved `SidecarConfig`.
    ///
    /// Used by integration tests that need to substitute a mock sidecar bundle
    /// path without setting an env var in production code. The rest of the
    /// build proceeds as normal: settings are loaded from
    /// `paths.config_dir()/settings.toml` (or defaults if the file is
    /// missing), and the sidecar supervisor is constructed from the provided
    /// `sidecar_config` instead of calling `resolve_sidecar_config`.
    /// `settings.subagent.pi_sidecar.enabled` must be `true` for the sidecar
    /// to be wired in (same gate as the production path).
    pub fn new_with_sidecar_config(
        db: Database,
        paths: BusytokPaths,
        sidecar_config: busytok_subagent::sidecar::SidecarConfig,
    ) -> Self {
        let adapters: Vec<BoxedAdapter> = vec![
            Box::new(busytok_adapters::ClaudeCodeAdapter),
            Box::new(busytok_adapters::CodexAdapter),
        ];
        let settings = BusytokSettings::load(&paths).unwrap_or_else(|e| {
            warn!("Failed to load settings, using defaults: {e}");
            BusytokSettings::default()
        });
        Self::build_with_sidecar_config(db, paths, adapters, settings, sidecar_config)
    }

    /// Shared constructor for `new` and `with_adapters`.
    fn build(db: Database, paths: BusytokPaths, adapters: Vec<BoxedAdapter>) -> Self {
        let settings = BusytokSettings::load(&paths).unwrap_or_else(|e| {
            warn!("Failed to load settings, using defaults: {e}");
            BusytokSettings::default()
        });
        Self::build_with_settings(db, paths, adapters, settings)
    }

    /// Shared constructor accepting pre-loaded settings.
    fn build_with_settings(
        db: Database,
        paths: BusytokPaths,
        adapters: Vec<BoxedAdapter>,
        settings: BusytokSettings,
    ) -> Self {
        let event_bus = AppEventBus::new(64);

        let db = Arc::new(Mutex::new(db));
        // Wrap settings in `Arc<Mutex>` ONCE here so the WorkerPool's
        // provider-lookup closure and the supervisor's `settings` field share
        // the SAME live store. Without this, the closure would capture a
        // snapshot taken at construction time, so `provider_update` (which
        // mutates `self.settings`) would NOT be visible to `ensure_worker`
        // when it re-spawns a worker.
        let settings = Arc::new(Mutex::new(settings));
        let sidecar = Self::construct_sidecar(&settings, &paths, &db, None);
        Self::assemble_with_sidecar(db, paths, adapters, settings, event_bus, sidecar)
    }

    /// Shared constructor accepting pre-loaded settings AND a pre-resolved
    /// `SidecarConfig`. Used by `new_with_sidecar_config` so integration tests
    /// can inject a mock sidecar bundle path without setting an env var in
    /// production code. Skips `resolve_sidecar_config` and uses the provided
    /// config directly when constructing the `PiSidecarSupervisor`.
    fn build_with_sidecar_config(
        db: Database,
        paths: BusytokPaths,
        adapters: Vec<BoxedAdapter>,
        settings: BusytokSettings,
        sidecar_config: busytok_subagent::sidecar::SidecarConfig,
    ) -> Self {
        let event_bus = AppEventBus::new(64);
        let db = Arc::new(Mutex::new(db));
        // See `build_with_settings`: wrap settings so the pool's provider
        // lookup reads live settings (provider_update visibility fix).
        let settings = Arc::new(Mutex::new(settings));
        let sidecar = Self::construct_sidecar(&settings, &paths, &db, Some(sidecar_config));
        Self::assemble_with_sidecar(db, paths, adapters, settings, event_bus, sidecar)
    }

    /// Construct the sidecar executor + pool + supervisor + init-error.
    /// When `sidecar_config_override` is `Some`, skip `resolve_sidecar_config`
    /// and use the provided config directly (test injection path). When
    /// `None`, resolve from settings + paths (production path).
    ///
    /// Returns a `SidecarRuntime` whose fields are `Some` only when the
    /// sidecar is enabled AND its config resolved successfully. The executor
    /// is `MockTaskExecutor` when disabled, `FailingTaskExecutor` when
    /// enabled-but-broken (with `sidecar_init_error` set).
    ///
    /// **Phase 3 Task 3:** the executor is rewired from a single
    /// `PiSidecarSupervisor` to `Arc<WorkerPool>`. The pool is constructed
    /// with a `HashMap<String, ProviderRuntimeEntry>` populated from SQL
    /// (the SQL-backed provider catalog — Task 7: the sidecar reads from
    /// SQL, not from settings TOML).
    /// Two-phase bootstrap:
    /// pool → executor → responder factory → `set_responder_factory` →
    /// `ensure_worker(first enabled provider)` → supervisor for the
    /// `sidecar_supervisor` field (used by doctor checks, shutdown, and
    /// runtime status). If no providers are configured, `sidecar_supervisor`
    /// stays `None` (degraded — delegate calls fail with "profile not bound
    /// to a provider"). The full multi-provider runtime status (showing all
    /// workers) is Task 4's scope.
    ///
    /// **Phase 5:** returns `SidecarComponents` (not a 7-tuple) so the same
    /// construction path is reused by `rebuild_sidecar_runtime` when
    /// `pi_sidecar_locator_update` flips `enabled` to `true` mid-flight.
    fn construct_sidecar(
        settings: &Arc<Mutex<BusytokSettings>>,
        paths: &BusytokPaths,
        db: &Arc<Mutex<Database>>,
        sidecar_config_override: Option<busytok_subagent::sidecar::SidecarConfig>,
    ) -> SidecarComponents {
        // Lock settings briefly for the reads below. `settings` is the SAME
        // `Arc<Mutex<BusytokSettings>>` shared with the supervisor's
        // `self.settings` field (constructed in `build_with_settings`), so
        // mutations made by `provider_update` are visible to the pool's
        // provider-lookup closure (which captures an `Arc::clone`).
        let pi_sidecar_enabled = settings.lock().unwrap().subagent.pi_sidecar.enabled;
        if !pi_sidecar_enabled {
            return SidecarComponents {
                executor: Arc::new(busytok_subagent::mock_executor::MockTaskExecutor)
                    as Arc<dyn busytok_subagent::mock_executor::TaskExecutor>,
                sidecar_supervisor: None,
                sidecar_init_error: None,
                pressure_gate: None,
                sidecar_executor: None,
                pressure_responder: None,
                worker_pool: None,
            };
        }
        // Either use the injected config (test path) or resolve the base
        // (unbound) config from settings + paths. `resolve_base_sidecar_config`
        // produces a config without provider-specific env — the pool injects
        // `OPENAI_API_KEY` / `OPENAI_BASE_URL` per provider via
        // `inject_provider_env` before constructing each supervisor.
        let config_result = match sidecar_config_override {
            Some(cfg) => Ok(cfg),
            None => {
                let pi_sidecar = settings.lock().unwrap().subagent.pi_sidecar.clone();
                busytok_subagent::sidecar::config::resolve_base_sidecar_config(&pi_sidecar, paths)
            }
        };
        match config_result {
            Ok(sidecar_config) => {
                let gate = Arc::new(busytok_subagent::PressureGate::new());

                // Build the providers map from SQL (Task 7: sidecar reads
                // from the SQL-backed provider catalog). Only enabled
                // providers with a non-None api_key are included;
                // `provider_changed` updates this map at runtime via
                // `update_provider_and_kill_old`.
                let mut first_runnable_provider: Option<String> = None;
                let providers: std::collections::HashMap<
                    String,
                    busytok_subagent::sidecar::ProviderRuntimeEntry,
                > = {
                    let db = db.lock().unwrap();
                    db.list_providers()
                        .map_err(|e| {
                            error!(
                                event_code = "subagent.sidecar.list_providers_failed",
                                error = %e,
                                "failed to list providers from SQL during construct_sidecar"
                            );
                        })
                        .unwrap_or_default()
                        .into_iter()
                        .filter_map(|s| {
                            let p = db.get_provider_with_secret(&s.id).ok()??;
                            if !p.enabled {
                                return None;
                            }
                            let api_key = p.api_key?;
                            // Capture the first runnable provider (in DB
                            // order) for eager bootstrap below. HashMap
                            // loses insertion order, so we capture here.
                            if first_runnable_provider.is_none() {
                                first_runnable_provider = Some(p.id.clone());
                            }
                            Some((
                                p.id.clone(),
                                busytok_subagent::sidecar::ProviderRuntimeEntry {
                                    provider_id: p.id,
                                    api_key,
                                    base_url: p.base_url,
                                },
                            ))
                        })
                        .collect()
                };

                // Build the pool. The base config is cloned per-provider by
                // `ensure_worker`, with env overridden (OPENAI_API_KEY +
                // OPENAI_BASE_URL injected via `inject_provider_env`).
                let resource_policy = settings.lock().unwrap().subagent.resource_policy.clone();
                let pool = Arc::new(busytok_subagent::sidecar::WorkerPool::new(
                    sidecar_config,
                    Some(Arc::clone(db)),
                    providers,
                    Some(Arc::clone(&gate)),
                    resource_policy,
                ));

                // Two-phase bootstrap step 1: construct the executor
                // (captures the pool). The executor is strong-owned here so
                // `PressureResponder` can hold a `Weak<SidecarTaskExecutor>`
                // without an Arc cycle.
                let exec_concrete =
                    Arc::new(busytok_subagent::sidecar::SidecarTaskExecutor::with_pool(
                        Arc::clone(&pool),
                        Some(Arc::clone(db)),
                    ));

                // Two-phase bootstrap step 2: construct the responder
                // factory (captures executor weak + gate + holder). The
                // factory is called by `ensure_worker` to construct a
                // `PressureResponder` per supervisor. The holder keeps all
                // responders alive (strong refs) so the `Weak` refs stored
                // on each supervisor stay upgradeable.
                let responder_holder: Arc<Mutex<Vec<Arc<busytok_subagent::PressureResponder>>>> =
                    Arc::new(Mutex::new(Vec::new()));
                let holder_for_factory = Arc::clone(&responder_holder);
                let exec_weak = Arc::downgrade(&exec_concrete);
                let gate_for_factory = Arc::clone(&gate);
                let factory: busytok_subagent::sidecar::ResponderFactory = Arc::new(
                    move |sup_weak: Weak<busytok_subagent::sidecar::PiSidecarSupervisor>| {
                        let responder = Arc::new(busytok_subagent::PressureResponder::new(
                            sup_weak,
                            exec_weak.clone(),
                            Arc::clone(&gate_for_factory),
                        ));
                        holder_for_factory
                            .lock()
                            .unwrap()
                            .push(Arc::clone(&responder));
                        responder
                    },
                );
                pool.set_responder_factory(factory);

                // Eagerly `ensure_worker` the first runnable provider so the
                // `sidecar_supervisor` field has a supervisor for doctor
                // checks, shutdown, and runtime status. If no providers are
                // configured (or all lack credentials), `sidecar_supervisor`
                // stays `None` — delegate calls will fail with "profile not
                // bound to a provider" or "unknown provider" (surfaced via
                // the executor's error path).
                let sidecar_supervisor =
                    first_runnable_provider.and_then(|pid| match pool.ensure_worker(&pid) {
                        Ok(sup) => Some(sup),
                        Err(e) => {
                            error!(
                                event_code = "subagent.sidecar.ensure_worker_failed",
                                provider_id = %pid,
                                error = %e,
                                "ensure_worker failed for first provider during construction"
                            );
                            None
                        }
                    });

                // Get the first responder from the holder (if ensure_worker
                // succeeded). This is stored in `BusytokSupervisor.pressure_responder`
                // so the accessor can return it; all responders (including
                // future ones from lazy `ensure_worker` calls) are kept alive
                // by the holder inside the factory closure.
                let pressure_responder = responder_holder.lock().unwrap().first().cloned();

                let exec: Arc<dyn busytok_subagent::mock_executor::TaskExecutor> =
                    Arc::clone(&exec_concrete)
                        as Arc<dyn busytok_subagent::mock_executor::TaskExecutor>;
                SidecarComponents {
                    executor: exec,
                    sidecar_supervisor,
                    sidecar_init_error: None,
                    pressure_gate: Some(gate),
                    sidecar_executor: Some(exec_concrete),
                    pressure_responder,
                    worker_pool: Some(Arc::clone(&pool)),
                }
            }
            Err(e) => {
                // `build_with_settings` returns `Self`, not `Result<Self>`,
                // so the error cannot propagate. Plan 2 requires that
                // `enabled=true` MUST use the sidecar — falling back to
                // MockTaskExecutor would mask a deployment misconfiguration
                // as "functional" (delegate succeeds with mock output).
                // Instead inject a FailingTaskExecutor that fails every
                // delegate call with a clear error, AND capture the reason
                // in `sidecar_init_error` for status reporting.
                let msg = e.to_string();
                error!(
                    event_code = "subagent.sidecar.config_resolve_failed",
                    error = %e,
                    "sidecar config resolve failed; injecting FailingTaskExecutor — delegate calls will fail"
                );
                SidecarComponents {
                    executor: Arc::new(busytok_subagent::mock_executor::FailingTaskExecutor {
                        reason: msg.clone(),
                    })
                        as Arc<dyn busytok_subagent::mock_executor::TaskExecutor>,
                    sidecar_supervisor: None,
                    sidecar_init_error: Some(msg),
                    pressure_gate: None,
                    sidecar_executor: None,
                    pressure_responder: None,
                    worker_pool: None,
                }
            }
        }
    }

    /// Assemble the final `BusytokSupervisor` from the shared constructor
    /// inputs plus the already-constructed sidecar components. Both
    /// `build_with_settings` and `build_with_sidecar_config` funnel through
    /// this to avoid duplicating the ~60 lines of manager/read-service/
    /// writer/event-bus wiring.
    fn assemble_with_sidecar(
        db: Arc<Mutex<Database>>,
        paths: BusytokPaths,
        adapters: Vec<BoxedAdapter>,
        settings: Arc<Mutex<BusytokSettings>>,
        event_bus: AppEventBus,
        components: SidecarComponents,
    ) -> Self {
        let read_service = {
            let db_guard = db.lock().unwrap();
            if let Some(path) = db_guard.path_buf() {
                crate::read_service::ReadService::new(path, 2)
            } else {
                crate::read_service::ReadService::new_in_memory(Arc::clone(&db), 1)
            }
        };

        busytok_pricing::init_catalog(Some(&paths.price_catalog_path()));

        let event_bus = Arc::new(event_bus);
        let status = Arc::new(tokio::sync::RwLock::new(ServiceStatusSnapshot::new()));
        // `settings` is already the shared `Arc<Mutex<BusytokSettings>>` —
        // shared with the WorkerPool's provider-lookup closure (live settings
        // visibility fix) and the supervisor's `self.settings` field.

        let source_registry = crate::source_registry::SourceRegistry::new(
            Arc::clone(&settings),
            Arc::clone(&db),
            Arc::clone(&event_bus),
        );

        let generation_manager = Arc::new(crate::generation_manager::GenerationManager::new(
            Arc::clone(&db),
            Arc::clone(&status),
        ));

        let (writer_handle, writer_join) = writer::try_spawn_writer(
            Arc::clone(&db),
            Arc::clone(&status),
            Arc::clone(&event_bus),
            Arc::clone(&settings),
            writer::DEFAULT_WRITER_CAPACITY,
        );

        let catalog_reload_join = try_spawn_catalog_reloader(paths.price_catalog_path().clone());

        // Construct the complete `SidecarRuntime` (manager + 6 fields) from
        // the components. Shared with `rebuild_sidecar_runtime` so the same
        // completion-hook + manager-construction path is used both at startup
        // and on mid-flight rebuild.
        let sidecar_runtime =
            Self::build_sidecar_runtime(components, &db, &settings, &generation_manager);

        // §8.3 step 2 "queue only" background dispatcher (Task 7 Finding 3
        // fix): spawn the dispatcher that polls `subagent_tasks` for queued
        // rows and executes them when the pressure gate is not paused. Uses
        // the sync-safe spawn pattern (mirrors `try_spawn_writer`): when no
        // Tokio runtime is active (e.g. sync unit tests that construct a
        // supervisor via `BusytokSupervisor::new()`), the handle + sender
        // are `None` and the dispatcher is not started. `shutdown_writer()`
        // and `Drop` both treat `None` as a no-op.
        let (dispatcher_handle, dispatcher_shutdown) =
            if tokio::runtime::Handle::try_current().is_ok() {
                let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
                let handle = sidecar_runtime
                    .subagent_manager
                    .spawn_task_dispatcher(shutdown_rx);
                (Some(handle), Some(shutdown_tx))
            } else {
                (None, None)
            };

        Self {
            db,
            event_bus,
            source_registry,
            generation_manager,
            adapters: Mutex::new(adapters),
            paths,
            last_scan_stats: Mutex::new(None),
            settings,
            status,
            writer_handle,
            read_service,
            sidecar_runtime: std::sync::RwLock::new(sidecar_runtime),
            rebuild_lock: tokio::sync::Mutex::new(()),
            _writer_join: Mutex::new(writer_join),
            _catalog_reload_join: Mutex::new(catalog_reload_join),
            task_dispatcher: Mutex::new(dispatcher_handle),
            dispatcher_shutdown: Mutex::new(dispatcher_shutdown),
        }
    }

    /// Build a complete `SidecarRuntime` from `SidecarComponents`: constructs
    /// the `SubagentManager` (which owns the executor) and installs the
    /// post-task completion hook (writes queued tasks' usage into the unified
    /// `usage_events` table). Shared between initial construction
    /// (`assemble_with_sidecar`) and mid-flight rebuild
    /// (`rebuild_sidecar_runtime`) so the manager wiring is identical in
    /// both paths.
    fn build_sidecar_runtime(
        components: SidecarComponents,
        db: &Arc<Mutex<Database>>,
        settings: &Arc<Mutex<BusytokSettings>>,
        generation_manager: &Arc<crate::generation_manager::GenerationManager>,
    ) -> SidecarRuntime {
        let subagent_settings = settings.lock().unwrap().subagent.clone();
        let subagent_manager = Arc::new(busytok_subagent::SubagentManager::with_pressure_gate(
            Arc::clone(db),
            subagent_settings,
            "pi",
            components.executor,
            components.pressure_gate.clone(),
        ));

        // P1 #2 fix: register a post-task completion hook so queued tasks'
        // usage flows into `usage_events` + rollups via the SAME
        // `bridge_subagent_usage` seam as synchronous `delegate()`.
        let hook_db = Arc::clone(db);
        let hook_gen = Arc::clone(generation_manager);
        let hook_settings = Arc::clone(settings);
        subagent_manager.set_task_completion_hook(Arc::new(
            move |result: &busytok_subagent::models::DelegateResult, cwd: &str| {
                // Read the active generation ID fresh on each invocation so a
                // later promotion is picked up by queued-task completion events.
                let gen_id = hook_gen.active_generation_id();
                if let Err(e) =
                    bridge_subagent_usage(&hook_db, gen_id.as_deref(), &hook_settings, result, cwd)
                {
                    tracing::warn!(
                        event_code = "subagent.usage_write_failed",
                        task_id = %result.task_id,
                        error = %e,
                        "dispatcher: failed to write subagent usage event to unified usage_events"
                    );
                }
            },
        ));

        SidecarRuntime {
            subagent_manager,
            sidecar_supervisor: components.sidecar_supervisor,
            worker_pool: components.worker_pool,
            pressure_gate: components.pressure_gate,
            sidecar_executor: components.sidecar_executor,
            pressure_responder: components.pressure_responder,
            sidecar_init_error: components.sidecar_init_error,
        }
    }

    /// Rebuild the entire sidecar runtime mid-flight after
    /// `pi_sidecar_locator_update` changed `enabled` or `runtime_dir`.
    /// Drains the old task dispatcher, constructs a fresh
    /// `SidecarRuntime` (new executor/pool/supervisor/manager), spawns a
    /// new dispatcher, and atomically swaps the runtime under the write
    /// lock. The old sidecar supervisor is shut down AFTER the swap so the
    /// lock is held for the minimum possible time.
    ///
    /// Spec §472-475: fresh-install closed loop. The GUI calls
    /// `pi_sidecar_locator_update` → this rebuild → `doctor`/`delegate`
    /// work in the CURRENT session (no service restart required).
    ///
    /// Cannot fail: `construct_sidecar` produces `SidecarComponents` (with
    /// a `FailingTaskExecutor` + `sidecar_init_error` if config resolution
    /// fails). The error is surfaced via `sidecar_init_error()` / doctor
    /// checks, not propagated.
    ///
    /// Serialized by `rebuild_lock` so concurrent `pi_sidecar_locator_update`
    /// RPCs can't leak dispatcher tasks (one call takes the handle, the
    /// other gets `None`, and the second call's new dispatcher overwrites
    /// the first's — leaking it as a detached task).
    ///
    /// # Invariants (lock ordering + async safety)
    ///
    /// 1. **`rebuild_lock` acquired first** (line below) — held for the
    ///    entire rebuild. Without it, two concurrent RPCs would race: one
    ///    takes the dispatcher handle, the other gets `None`, the second's
    ///    new dispatcher overwrites the first's — leaking a detached task.
    /// 2. **`rebuild_lock` is `tokio::sync::Mutex<()>`** — NOT `std::sync`
    ///    because we hold the guard across `.await` points (steps 1 and 7
    ///    below). `std::sync::Mutex` would deadlock under Tokio.
    /// 3. **`sidecar_runtime` write lock held only for the pointer swap**
    ///    (step 5) — released BEFORE step 7's `old_sup.shutdown().await`.
    ///    Never hold `std::sync::RwLock` guard across `.await`.
    /// 4. **Dispatcher handle stored before old supervisor shut down**
    ///    (step 6 before step 7) — synchronous steps 2–6 have no `.await`
    ///    between draining the old dispatcher and storing the new one, so
    ///    no other async task observes the intermediate `None` state.
    /// 5. **Old supervisor shut down AFTER lock release** (step 7) —
    ///    best-effort; failures are logged but don't propagate (the new
    ///    runtime is already live).
    async fn rebuild_sidecar_runtime(&self) {
        let _rebuild_guard = self.rebuild_lock.lock().await;

        info!(
            event_code = "pi_sidecar.rebuild_start",
            "starting sidecar runtime rebuild"
        );

        // 1. Drain the old task dispatcher (stop polling for queued tasks).
        //    Mirrors `shutdown_writer`: send `true` on the watch channel +
        //    await the JoinHandle. No-op when no Tokio runtime was active at
        //    construction time (both fields are `None`).
        let old_dispatcher_handle = self.task_dispatcher.lock().unwrap().take();
        let old_dispatcher_shutdown = self.dispatcher_shutdown.lock().unwrap().take();
        if let Some(tx) = old_dispatcher_shutdown {
            let _ = tx.send(true);
        }
        if let Some(handle) = old_dispatcher_handle {
            let _ = handle.await;
        }

        // 2. Construct fresh components from the CURRENT (already-swapped)
        //    settings. `construct_sidecar` reads `self.settings` which was
        //    updated by `pi_sidecar_locator_update` BEFORE calling this.
        let components = Self::construct_sidecar(&self.settings, &self.paths, &self.db, None);

        // 3. Build the new SidecarRuntime (SubagentManager + completion hook).
        //    Shared with `assemble_with_sidecar` so the wiring is identical.
        let new_runtime = Self::build_sidecar_runtime(
            components,
            &self.db,
            &self.settings,
            &self.generation_manager,
        );

        // Capture the init_error for observability before the runtime is
        // moved into the RwLock.
        let new_init_error = new_runtime.sidecar_init_error.clone();

        // 4. Spawn the new task dispatcher (only when a Tokio runtime is
        //    active — mirrors `assemble_with_sidecar`). In practice this
        //    is always true since `rebuild_sidecar_runtime` is called from
        //    the async `pi_sidecar_locator_update` RPC handler.
        let (new_handle, new_shutdown) = if tokio::runtime::Handle::try_current().is_ok() {
            let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
            let handle = new_runtime
                .subagent_manager
                .spawn_task_dispatcher(shutdown_rx);
            (Some(handle), Some(shutdown_tx))
        } else {
            (None, None)
        };

        // 5. Swap the runtime under the write lock. Extract the old runtime
        //    so its sidecar supervisor can be shut down AFTER the lock is
        //    released (never hold a `std::sync::RwLock` guard across `.await`).
        let old_runtime = {
            let mut guard = self.sidecar_runtime.write().unwrap();
            std::mem::replace(&mut *guard, new_runtime)
        };

        // 6. Store the new dispatcher handle + shutdown sender (replacing
        //    the `None` left by step 1's `take()`). Steps 2–6 are all
        //    synchronous — no `.await` between draining the old dispatcher
        //    and storing the new one, so no other async task can observe
        //    the intermediate `None` state.
        *self.task_dispatcher.lock().unwrap() = new_handle;
        *self.dispatcher_shutdown.lock().unwrap() = new_shutdown;

        // 7. Shut down the OLD sidecar supervisor(s) (async, after lock
        //    release). Best-effort: failures are logged but don't propagate —
        //    the new runtime is already live.
        //
        //    Multi-provider fix: prefer `pool.shutdown_all()` to drain ALL
        //    workers (including lazily-spawned ones from delegate calls),
        //    not just the single "first enabled provider" supervisor.
        //    Without this, flipping `enabled` (or disabling the sidecar)
        //    would orphan Node subprocesses for the other providers,
        //    leaking stale-credential children. Falls back to single-
        //    supervisor shutdown only when no pool exists (degraded mode
        //    where construct_sidecar failed before pool creation).
        if let Some(old_pool) = old_runtime.worker_pool.as_ref() {
            old_pool.shutdown_all().await;
        } else if let Some(old_sup) = old_runtime.sidecar_supervisor {
            if let Err(e) = old_sup.shutdown().await {
                warn!(
                    event_code = "subagent.sidecar.rebuild_old_shutdown_failed",
                    error = %e,
                    "failed to shut down old sidecar supervisor during rebuild (best-effort)"
                );
            }
        }

        info!(
            event_code = "pi_sidecar.rebuild_complete",
            degraded = new_init_error.is_some(),
            init_error = ?new_init_error,
            "sidecar runtime rebuild complete"
        );
    }

    /// Discover log sources using current settings and user roots from DB.
    fn discover_sources(&self) -> Result<Vec<busytok_discovery::DiscoveredLogSource>> {
        self.source_registry.discover_all()
    }

    /// Returns the error message captured when `pi_sidecar.enabled = true`
    /// but `resolve_sidecar_config` failed at construction time (or during a
    /// mid-flight rebuild), OR `None` when the sidecar initialized
    /// successfully or was not enabled.
    ///
    /// This lets Task 6 / status reporting surface sidecar config degradation
    /// without refactoring `build_with_settings` to return `Result<Self>`.
    /// The service is still running in degraded mode (FailingTaskExecutor)
    /// when this returns `Some`; callers should treat a non-`None` value as a
    /// loud signal that the configured sidecar is NOT backing delegate calls.
    ///
    /// Returns an owned `String` (not `&str`) because the runtime is behind a
    /// `RwLock` — a reference into the guard would not outlive the lock.
    pub fn sidecar_init_error(&self) -> Option<String> {
        self.sidecar_runtime
            .read()
            .unwrap()
            .sidecar_init_error
            .clone()
    }

    /// §8.3 step 2: pressure gate shared with `SubagentManager`. `None` when
    /// the sidecar is disabled (no pressure response chain). Task 6 / status
    /// reporting can read this to expose the current pressure action via
    /// `gate.last_action()`.
    ///
    /// Returns an owned `Arc` (not `&Arc`) because the runtime is behind a
    /// `RwLock` — callers clone the Arc out under a short read lock.
    pub fn pressure_gate(&self) -> Option<Arc<busytok_subagent::PressureGate>> {
        self.sidecar_runtime.read().unwrap().pressure_gate.clone()
    }

    /// §8.3 escalation chain driver. `None` when the sidecar is disabled.
    /// Task 6 / status reporting can read this to surface responder state.
    pub fn pressure_responder(&self) -> Option<Arc<busytok_subagent::PressureResponder>> {
        self.sidecar_runtime
            .read()
            .unwrap()
            .pressure_responder
            .clone()
    }

    /// Pi sidecar supervisor handle. `None` when the sidecar is disabled.
    /// Exposed so tests (and future status reporting) can drive
    /// `ensure_started()` / `shutdown()` and read `worker_snapshot()` to
    /// cover the `WorkerState::Running` branch of `subagent_runtime_status`.
    pub fn sidecar_supervisor(
        &self,
    ) -> Option<Arc<busytok_subagent::sidecar::PiSidecarSupervisor>> {
        self.sidecar_runtime
            .read()
            .unwrap()
            .sidecar_supervisor
            .clone()
    }

    /// Multi-provider worker pool handle. `None` when the sidecar is
    /// disabled or config resolution failed. Exposed so tests can drive
    /// `pool.ensure_worker(pid)` / `pool.remove_worker_and_kill(pid)` and
    /// read `pool.worker_snapshots()` to cover the multi-provider
    /// aggregation branch of `subagent_runtime_status` (Phase 3 Task 4).
    pub fn worker_pool(&self) -> Option<Arc<busytok_subagent::sidecar::WorkerPool>> {
        self.sidecar_runtime.read().unwrap().worker_pool.clone()
    }

    /// Phase 3 Task 4 (P1b fix) + Task 7 (SQL-backed provider_changed):
    /// Re-read the provider from SQL and either update the pool's
    /// `ProviderRuntimeEntry` + kill the old worker (provider enabled with
    /// api_key) or remove the provider entry + kill the worker (provider
    /// disabled / missing / no api_key). Both paths use async kill methods
    /// (`update_provider_and_kill_old` / `remove_worker_and_kill`) so the
    /// old sidecar child process is explicitly `force_kill`'d (pool.rs:20-24
    /// invariant — no `Drop` fallback).
    ///
    /// Called by `provider_update` (covers both metadata changes AND key
    /// rotations) and `provider_create` (defensive — typically a no-op
    /// since no worker exists yet for a brand-new provider).
    pub async fn provider_changed(&self, provider_id: &str) {
        let Some(pool) = self.worker_pool() else {
            debug!(
                event_code = "subagent.provider_changed_noop",
                provider_id = %provider_id,
                "provider_changed called but sidecar is disabled — no-op"
            );
            return;
        };
        // Re-read provider from SQL to decide update vs remove.
        let entry = {
            let db = self.db.lock().unwrap();
            db.get_provider_with_secret(provider_id).ok().flatten()
        };
        match entry {
            Some(p) if p.enabled && p.api_key.is_some() => {
                info!(
                    event_code = "subagent.provider_changed",
                    provider_id = %provider_id,
                    "provider changed — updating entry + killing old worker for lazy re-spawn"
                );
                if let Err(e) = pool
                    .update_provider_and_kill_old(busytok_subagent::sidecar::ProviderRuntimeEntry {
                        provider_id: p.id,
                        api_key: p.api_key.unwrap(),
                        base_url: p.base_url,
                    })
                    .await
                {
                    warn!(
                        event_code = "subagent.provider_changed_kill_failed",
                        provider_id = %provider_id,
                        error = %e,
                        "failed to update+kill worker after provider change (best-effort)"
                    );
                }
            }
            _ => {
                // Provider disabled / missing / no api_key — remove the
                // provider entry (so ensure_worker won't re-spawn with
                // stale credentials) AND kill + remove the worker. The
                // entry is removed BEFORE the kill so a concurrent
                // ensure_worker can't re-spawn between the two steps.
                // NOTE: this is distinct from the auth-fail kill path in
                // the executor, which calls remove_worker_and_kill ONLY
                // (provider entry stays so the next ensure_worker can
                // re-spawn after a credential refresh).
                info!(
                    event_code = "subagent.provider_changed_remove",
                    provider_id = %provider_id,
                    "provider changed (disabled/missing/no-key) — removing worker + provider entry"
                );
                pool.remove_provider_entry(provider_id);
                if let Err(e) = pool.remove_worker_and_kill(provider_id).await {
                    warn!(
                        event_code = "subagent.provider_changed_kill_failed",
                        provider_id = %provider_id,
                        error = %e,
                        "failed to kill worker after provider change (best-effort)"
                    );
                }
            }
        }
    }

    /// Phase 3 Task 4 (P1b fix): kill + remove a single provider's worker
    /// after the provider is deleted. Same mechanism as `provider_changed`
    /// but a distinct log event code so audit trails can distinguish
    /// "changed" from "deleted". Called by `provider_delete` AFTER the SQL
    /// delete succeeds. Removes the provider entry from the pool's map
    /// (so ensure_worker won't re-spawn) AND kills + removes the worker.
    pub async fn provider_deleted(&self, provider_id: &str) {
        if let Some(pool) = self.worker_pool() {
            info!(
                event_code = "subagent.provider_deleted",
                provider_id = %provider_id,
                "provider deleted — killing worker (if any) to release resources"
            );
            // Remove the provider entry BEFORE the kill so a concurrent
            // ensure_worker can't re-spawn between the two steps.
            pool.remove_provider_entry(provider_id);
            if let Err(e) = pool.remove_worker_and_kill(provider_id).await {
                warn!(
                    event_code = "subagent.provider_deleted_kill_failed",
                    provider_id = %provider_id,
                    error = %e,
                    "failed to kill worker after provider delete (best-effort)"
                );
            }
        } else {
            debug!(
                event_code = "subagent.provider_deleted_noop",
                provider_id = %provider_id,
                "provider_deleted called but sidecar is disabled — no-op"
            );
        }
    }

    /// Run the 11 spec §7.1 doctor checks. The subagent-specific checks
    /// (SQLite, sidecar launchable, resource policy, stale subagents) are
    /// real; the 6 bundle-inspection checks probe the filesystem + sidecar
    /// supervisor. `overall_ok` is true iff no check has `status == "error"`
    /// (warnings don't fail). Async because the `protocol_version` check
    /// does a short-lived probe (`ensure_started().await` +
    /// `shutdown_internal().await`) when the sidecar is enabled but not
    /// running.
    async fn run_subagent_doctor(&self) -> SubagentDoctorResultDto {
        let mut checks: Vec<DoctorCheckDto> = Vec::new();

        // 1. busytok-service running — always ok (we're running this code).
        checks.push(DoctorCheckDto {
            name: "service_running".into(),
            status: "ok".into(),
            detail: None,
        });

        // 2. SQLite readable/writable + schema version (spec §7.1).
        //    Three probes: SELECT 1 (readable), BEGIN IMMEDIATE; ROLLBACK
        //    (writable — fails on read-only DBs or locked state), and
        //    schema_version == SCHEMA_VERSION (correct migration applied).
        {
            let db = self.db.lock().unwrap();
            let readable = db
                .conn()
                .query_row("SELECT 1", [], |row| row.get::<_, i64>(0))
                .is_ok();
            let db_version: i64 = db
                .conn()
                .query_row(
                    "SELECT COALESCE(MAX(version), 0) FROM _schema_version",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap_or(0);
            let schema_version_ok = db_version == busytok_store::schema::SCHEMA_VERSION as i64;
            // Write probe: BEGIN IMMEDIATE + a no-op DELETE, then rollback.
            // The DELETE matches no rows (version=-999 never exists) but
            // forces SQLite to acquire a write lock and attempt I/O, which
            // fails on read-only connections (SQLITE_READONLY). A bare
            // `BEGIN IMMEDIATE; ROLLBACK;` is insufficient because in WAL
            // mode some SQLite builds allow acquiring a RESERVED lock on a
            // read-only connection without actually writing.
            //
            // Uses an RAII `Transaction` instead of `execute_batch` so the
            // transaction is rolled back on BOTH the success and failure
            // paths of the DELETE. With `execute_batch("BEGIN; DELETE;
            // ROLLBACK;")`, if DELETE fails (e.g. mid-transaction I/O
            // error), the batch aborts at the failing statement and the
            // trailing ROLLBACK never runs — leaving the connection in an
            // open transaction that pollutes subsequent operations. The
            // RAII `Transaction` guarantees cleanup via Drop regardless of
            // how the block exits.
            let writable = match rusqlite::Transaction::new_unchecked(
                db.conn(),
                rusqlite::TransactionBehavior::Immediate,
            ) {
                Ok(tx) => {
                    let delete_ok = tx
                        .execute("DELETE FROM _schema_version WHERE version = -999", [])
                        .is_ok();
                    // Explicit rollback — Drop would also do it, but be
                    // explicit so the cleanup guarantee is visible.
                    let _ = tx.rollback();
                    delete_ok
                }
                Err(_) => false, // BEGIN IMMEDIATE failed (read-only or locked)
            };
            let ok = readable && writable && schema_version_ok;
            let mut detail = format!(
                "schema_version={db_version} (expected {}), readable={readable}, writable={writable}",
                busytok_store::schema::SCHEMA_VERSION
            );
            if !ok {
                if !readable {
                    detail.push_str(" — SELECT 1 failed");
                } else if !writable {
                    detail.push_str(" — write probe failed (read-only or locked)");
                } else if !schema_version_ok {
                    detail.push_str(" — schema version mismatch");
                }
            }
            checks.push(DoctorCheckDto {
                name: "sqlite_readable".into(),
                status: if ok { "ok" } else { "error" }.into(),
                detail: Some(detail),
            });
        }

        // 3. Pi sidecar launchable — surface sidecar_init_error if present.
        //    When pi_sidecar.enabled=false, this is "ok" (feature off).
        let init_error = self.sidecar_init_error();
        checks.push(DoctorCheckDto {
            name: "sidecar_launchable".into(),
            status: if init_error.is_some() { "error" } else { "ok" }.into(),
            detail: init_error,
        });

        // 4-9. Bundled node arch, manifest, protocol version, model config,
        //      Pi runtime, artifact store — real probes (spec §7.1 lines
        //      865-870). Extract all settings-derived values BEFORE any
        //      `.await` — `self.settings` is a `std::sync::Mutex` and holding
        //      its guard across `.await` (protocol_version probe below) is
        //      forbidden. `runtime_dir` is cloned out as an owned value so
        //      the lock is released before the protocol probe.
        let runtime_dir = {
            let settings = self.settings.lock().unwrap();
            settings.subagent.pi_sidecar.runtime_dir.clone()
        };
        let runtime_dir_ref = runtime_dir.as_deref();

        // 4. Bundled Node architecture matches (spec §7.1 line 865).
        {
            let node_path = self.paths.sidecar_bundled_node_path(runtime_dir_ref);
            let expected_arch = std::env::consts::ARCH;
            let arch_ok = node_path
                .parent()
                .and_then(|p| p.file_name())
                .and_then(|n| n.to_str())
                .map(|n| n == expected_arch)
                .unwrap_or(false);
            let node_exists = node_path.exists();
            let ok = arch_ok && node_exists;
            let detail = if !node_exists {
                format!("bundled node not found at {}", node_path.display())
            } else if !arch_ok {
                format!("arch mismatch: expected {expected_arch}")
            } else {
                format!("ok ({expected_arch})")
            };
            checks.push(DoctorCheckDto {
                name: "bundled_node_arch".into(),
                status: if ok { "ok" } else { "error" }.into(),
                detail: Some(detail),
            });
        }

        // 5. Bundle manifest readable (spec §7.1 line 866, §5.1 line 549).
        //    Verifies manifest.json EXISTS, is READABLE, is PARSEABLE JSON, AND
        //    conforms to the SidecarManifest schema (version/protocol_version/
        //    bundle/node_runtime_version all present + correct types). A missing
        //    or malformed manifest is an "error" — the sidecar cannot be launched
        //    without a valid manifest.
        {
            let manifest_path = self.paths.sidecar_manifest_path(runtime_dir_ref);
            let (status, detail) =
                match std::fs::read_to_string(&manifest_path) {
                    Ok(contents) => {
                        match serde_json::from_str::<busytok_config::SidecarManifest>(&contents) {
                            Ok(m) => (
                                "ok",
                                format!(
                            "manifest readable (version={}, protocol_version={}, node={}): {}",
                            m.version, m.protocol_version, m.node_runtime_version,
                            manifest_path.display()
                        ),
                            ),
                            Err(e) => {
                                tracing::warn!(
                                    event_code = "subagent.doctor.manifest_invalid",
                                    path = %manifest_path.display(),
                                    error = %e,
                                    "manifest at path is not a valid SidecarManifest"
                                );
                                (
                                    "error",
                                    format!(
                                        "manifest at {} is not a valid SidecarManifest: {}",
                                        manifest_path.display(),
                                        e
                                    ),
                                )
                            }
                        }
                    }
                    Err(e) => {
                        tracing::warn!(
                            event_code = "subagent.doctor.manifest_unreadable",
                            path = %manifest_path.display(),
                            error = %e,
                            "manifest not readable at path"
                        );
                        (
                            "error",
                            format!(
                                "manifest not readable at {}: {}",
                                manifest_path.display(),
                                e
                            ),
                        )
                    }
                };
            checks.push(DoctorCheckDto {
                name: "bundle_manifest_readable".into(),
                status: status.into(),
                detail: Some(detail),
            });
        }

        // 6. Protocol version matches (spec §7.1 line 867).
        //    If sidecar is already running, protocol was verified during
        //    `adapter.initialize` in `ensure_started` → "ok". If not running,
        //    do a SHORT-LIVED PROBE: `ensure_started()` (spawns + verifies
        //    protocol via adapter.initialize), then `shutdown_internal()`.
        //    When `sidecar_supervisor` is None, distinguish three sub-cases:
        //      (a) `sidecar_init_error = Some` → "error" (enabled but broken).
        //      (b) `enabled = false` in settings → "warning" (disabled).
        //      (c) `enabled = true` but no supervisor (no providers configured
        //          or rebuild produced no worker) → "warning".
        //    Phase 5: the "pending restart" branch was removed —
        //    `pi_sidecar_locator_update` now rebuilds the sidecar runtime
        //    in-flight, so `enabled=true` always has a corresponding
        //    supervisor (or an init_error explaining why not).
        {
            let expected_pv = busytok_subagent::sidecar::protocol::PROTOCOL_VERSION;
            let sidecar_sup = self.sidecar_supervisor();
            let (status, detail) = match &sidecar_sup {
                Some(sup) if sup.try_is_running() => (
                    "ok",
                    format!(
                        "protocol_version={expected_pv}, sidecar running (verified during init)"
                    ),
                ),
                Some(sup) => match sup.ensure_started().await {
                    Ok(_handle) => {
                        if let Err(e) = sup.shutdown_internal().await {
                            warn!(
                                event_code = "subagent.doctor.protocol_probe_shutdown_failed",
                                error = %e,
                                "short-lived probe shutdown failed"
                            );
                        }
                        (
                            "ok",
                            format!(
                                "protocol_version={expected_pv}, verified via short-lived probe"
                            ),
                        )
                    }
                    Err(e) => (
                        "error",
                        format!("protocol probe failed (ensure_started): {e}"),
                    ),
                },
                None => {
                    if let Some(err) = self.sidecar_init_error() {
                        (
                            "error",
                            format!("protocol probe failed — sidecar not constructed: {err}"),
                        )
                    } else {
                        let enabled = self.settings.lock().unwrap().subagent.pi_sidecar.enabled;
                        if enabled {
                            warn!(
                                event_code = "subagent.doctor.protocol_version_no_supervisor",
                                "pi_sidecar enabled but no sidecar supervisor \
                                 (no providers configured or worker spawn failed) — \
                                 cannot probe protocol version"
                            );
                            (
                                "warning",
                                "pi_sidecar enabled but no sidecar worker running \
                                 — configure a provider first"
                                    .into(),
                            )
                        } else {
                            (
                                "warning",
                                "pi_sidecar disabled — cannot probe protocol version".into(),
                            )
                        }
                    }
                }
            };
            checks.push(DoctorCheckDto {
                name: "protocol_version".into(),
                status: status.into(),
                detail: Some(detail),
            });
        }

        // 7. Pi runtime installed (spec §7.1 line 869).
        //    (Task 3 removed the `default_model_config` doctor check —
        //    `SubagentModelsConfig` was deleted; provider/model binding is
        //    now per-subagent via SQL catalog, validated at delegate time.)
        {
            let node_path = self.paths.sidecar_bundled_node_path(runtime_dir_ref);
            let bundle_path = self.paths.sidecar_bundle_path(runtime_dir_ref);
            let ok = node_path.exists() && bundle_path.exists();
            let detail = if ok {
                "node + bundle present".to_string()
            } else {
                format!(
                    "missing: node={} bundle={}",
                    node_path.exists(),
                    bundle_path.exists()
                )
            };
            checks.push(DoctorCheckDto {
                name: "pi_runtime_installed".into(),
                status: if ok { "ok" } else { "error" }.into(),
                detail: Some(detail),
            });
        }

        // 9. Artifact store writable (spec §7.1 line 870).
        //    Self-heal: create the artifacts dir if missing so the probe
        //    tests actual writability rather than reporting a stale "missing"
        //    state. A missing dir that can't be created is reported as
        //    "not writable" — what the user cares about is whether artifacts
        //    can be written, not whether the dir pre-existed.
        {
            let artifacts_dir = self.paths.artifacts_dir();
            let dir_created =
                artifacts_dir.exists() || std::fs::create_dir_all(&artifacts_dir).is_ok();
            let probe_ok = if dir_created {
                let probe_path = artifacts_dir.join(".busytok_doctor_probe");
                std::fs::write(&probe_path, b"probe").is_ok()
                    && std::fs::remove_file(&probe_path).is_ok()
            } else {
                false
            };
            let detail = if probe_ok {
                format!("writable ({})", artifacts_dir.display())
            } else {
                format!("not writable: {}", artifacts_dir.display())
            };
            checks.push(DoctorCheckDto {
                name: "artifact_store_writable".into(),
                status: if probe_ok { "ok" } else { "error" }.into(),
                detail: Some(detail),
            });
        }

        // 10. Resource policy valid — check the deserialized policy fields.
        {
            let settings = self.settings.lock().unwrap();
            let p = &settings.subagent.resource_policy;
            let ok = p.memory_pressure_free_mb > 0 && p.monitor_interval_seconds > 0;
            checks.push(DoctorCheckDto {
                name: "resource_policy_valid".into(),
                status: if ok { "ok" } else { "error" }.into(),
                detail: Some(format!(
                    "memory_pressure_free_mb={}, monitor_interval_seconds={}",
                    p.memory_pressure_free_mb, p.monitor_interval_seconds
                )),
            });
        }

        // 11. Subagents unused > 30 days (warning, not error). SQL errors
        //     (table missing, DB locked) must NOT be swallowed into a
        //     false-green "ok" — surface them as "error" so overall_ok
        //     reflects the real state, mirroring the `sqlite_readable`
        //     check's `is_ok()` pattern above.
        {
            let db = self.db.lock().unwrap();
            let threshold_ms = now_ms() - (30 * 24 * 60 * 60 * 1000);
            let stale_result: rusqlite::Result<Vec<String>> = db
                .conn()
                .prepare(
                    "SELECT id FROM subagent_logical_subagents \
                     WHERE last_active_at_ms IS NOT NULL \
                     AND last_active_at_ms < ?1 \
                     AND status != 'deleted'",
                )
                .and_then(|mut stmt| {
                    let rows = stmt.query_map(rusqlite::params![threshold_ms], |row| {
                        row.get::<_, String>(0)
                    })?;
                    rows.collect::<rusqlite::Result<Vec<_>>>()
                });
            match stale_result {
                Ok(stale) => {
                    let is_warning = !stale.is_empty();
                    checks.push(DoctorCheckDto {
                        name: "subagents_unused_30d".into(),
                        status: if is_warning { "warning" } else { "ok" }.into(),
                        detail: if is_warning {
                            Some(format!(
                                "{} stale subagent(s): {}",
                                stale.len(),
                                stale.join(", ")
                            ))
                        } else {
                            None
                        },
                    });
                }
                Err(e) => {
                    checks.push(DoctorCheckDto {
                        name: "subagents_unused_30d".into(),
                        status: "error".into(),
                        detail: Some(format!("SQL error: {e}")),
                    });
                }
            }
        }

        let overall_ok = checks.iter().all(|c| c.status != "error");
        SubagentDoctorResultDto { checks, overall_ok }
    }

    // ── Scan methods ────────────────────────────────────────────────────
    //
    // Two families of scan entry points:
    //
    //   async (production) — `run_initial_scan`. These send
    //     writes through the bounded writer actor via `scan_once_via_writer`
    //     so that the single-write-owner architecture is preserved.
    //
    //   sync (test compat) — `run_scan_with_sources`,
    //     `run_initial_scan_with_sources`. These call
    //     `scan_once` which writes directly to the DB. They exist solely
    //     for synchronous test contexts where no Tokio runtime is active.

    fn detached_or_shared_database(&self) -> Result<DatabaseHandle<'_>> {
        let detached = {
            let db = self.db.lock().unwrap();
            db.reopen()?
        };

        if let Some(db) = detached {
            Ok(DatabaseHandle::Detached(db))
        } else {
            Ok(DatabaseHandle::Shared(self.db.lock().unwrap()))
        }
    }

    fn scan_database(&self) -> Result<DatabaseHandle<'_>> {
        self.detached_or_shared_database()
    }

    fn read_query_database(&self) -> Result<DatabaseHandle<'_>> {
        let detached = {
            let db = self.db.lock().unwrap();
            db.reopen_readonly()?
        };

        if let Some(db) = detached {
            Ok(DatabaseHandle::Detached(db))
        } else {
            Ok(DatabaseHandle::Shared(self.db.lock().unwrap()))
        }
    }

    fn prompt_database(&self) -> Result<DatabaseHandle<'_>> {
        self.detached_or_shared_database()
    }

    fn ensure_active_generation_for_existing_events(&self) -> Result<Option<String>> {
        self.generation_manager
            .ensure_active_generation_for_existing_events()
    }

    async fn mark_ready_exact_if_generation_valid(&self, gen_id: &str) -> Result<bool> {
        self.generation_manager
            .mark_ready_exact_if_generation_valid(gen_id)
            .await
    }

    /// Scan sources through a fresh database handle.
    ///
    /// Auto-creates an active generation if none exists. Production callers
    /// should prefer the async writer-actor path when a writer is available.
    fn run_detached_scan_for_sources(
        &self,
        sources: &[busytok_discovery::DiscoveredLogSource],
    ) -> Result<ScanStats> {
        let db = self.scan_database()?;
        let adapters = self
            .adapters
            .lock()
            .unwrap()
            .iter()
            .map(|a| a.clone_boxed())
            .collect::<Vec<_>>();
        let timezone = self.settings.lock().unwrap().timezone.clone();
        let rtz = ReportingTimezone::parse(&timezone)?;

        // Obtain or create a real generation so scanned data is visible
        // through the active-generation Overview read path.
        let gen_id = match self.generation_manager.active_generation_id() {
            Some(id) => id,
            None => {
                let new_id = format!("gen-{}", busytok_domain::now_ms());
                crate::rebuild::create_generation(&db, &new_id)?;
                self.generation_manager
                    .activate_generation(new_id.clone())?;
                new_id
            }
        };

        let stats = scan_once(&db, &adapters, sources, &self.event_bus, &rtz, &gen_id)?;
        *self.last_scan_stats.lock().unwrap() = Some(stats.clone());
        Ok(stats)
    }

    /// Perform an initial historical scan of all discovered sources.
    ///
    /// Production startup uses the async writer-actor path so the writer
    /// actor remains the sole owner of SQLite writes during bootstrap.
    /// Register discovered log sources and their files for live tailing
    /// without parsing historical content.  Each file's checkpoint offset is
    /// set to the current file size so the tailer only picks up new content.
    ///
    /// Used on fresh installs where there is no existing data to preserve.
    pub async fn register_new_install_sources(&self) -> Result<ScanStats> {
        let sources = self.discover_sources()?;

        let gen_id = format!("gen-{}", busytok_domain::now_ms());
        self.writer_handle
            .send(writer::WriteCommand::GenerationCreate(
                writer::GenerationCreateCommand {
                    generation_id: gen_id.clone(),
                },
            ))
            .await
            .map_err(|e| anyhow::anyhow!("writer channel send error: {e}"))?;
        self.writer_handle
            .flush()
            .await
            .context("failed to flush generation create")?;

        let mut total_files = 0usize;
        let now_ms = busytok_domain::now_ms();
        for source in &sources {
            // Register the log source so source_health_summary can be populated.
            let source_row = busytok_store::repository::LogSourceRow {
                id: source.source_id.clone(),
                agent: source.agent.as_str().to_string(),
                source_type: match source.source_type {
                    busytok_domain::LogSourceType::Jsonl => "jsonl",
                    busytok_domain::LogSourceType::SQLite => "sqlite",
                    busytok_domain::LogSourceType::Directory => "directory",
                }
                .to_string(),
                root_path: source.root_path.display().to_string(),
                configured_by_user: source.configured_by_user as i32,
                default_discovery_enabled: 1,
                status: "active".to_string(),
                last_scan_started_at_ms: Some(now_ms),
                last_scan_completed_at_ms: None,
                last_error: None,
                first_seen_at_ms: now_ms,
                last_seen_at_ms: now_ms,
                created_at_ms: now_ms,
                updated_at_ms: now_ms,
            };
            self.writer_handle
                .send(writer::WriteCommand::LogSourceUpsert(
                    writer::LogSourceUpsertCommand { row: source_row },
                ))
                .await
                .map_err(|e| anyhow::anyhow!("writer channel send error: {e}"))?;

            for file_path in &source.files {
                let file_id = crate::scan::derive_file_id(file_path);
                let size = std::fs::metadata(file_path).map(|m| m.len()).unwrap_or(0);
                let inode = busytok_tailer::read_inode(file_path);

                self.writer_handle
                    .send(writer::WriteCommand::TailBatch(writer::TailBatchCommand {
                        source_id: source.source_id.clone(),
                        source_file_id: Some(file_id),
                        source_file_agent: source.agent.as_str().to_string(),
                        source_file_path: file_path.to_string_lossy().to_string(),
                        source_file_inode: inode,
                        events: vec![],
                        tool_events: vec![],
                        diagnostic_events: vec![],
                        codex_snapshots: vec![],
                        generation_id: gen_id.clone(),
                        checkpoint_offset: Some(size),
                        write_policy: busytok_domain::UsageWritePolicy::InsertOnce,
                    }))
                    .await
                    .map_err(|e| anyhow::anyhow!("writer channel send error: {e}"))?;
                total_files += 1;
            }
        }

        self.writer_handle
            .flush()
            .await
            .context("failed to flush source registrations")?;

        // Promote generation so the service enters ReadyExact immediately.
        self.writer_handle
            .send(writer::WriteCommand::PromotionBarrier(
                writer::PromotionBarrierCommand {
                    from_generation_id: String::new(),
                    to_generation_id: gen_id.clone(),
                },
            ))
            .await
            .map_err(|e| anyhow::anyhow!("writer channel send error: {e}"))?;
        self.writer_handle
            .flush()
            .await
            .context("failed to flush promotion barrier")?;

        // Mark all sources as scan-completed so scan_state_from_conn sees
        // them as "completed" rather than perpetually "idle".
        let completed_ms = busytok_domain::now_ms();
        for source in &sources {
            let completion_row = busytok_store::repository::LogSourceRow {
                id: source.source_id.clone(),
                agent: source.agent.as_str().to_string(),
                source_type: match source.source_type {
                    busytok_domain::LogSourceType::Jsonl => "jsonl",
                    busytok_domain::LogSourceType::SQLite => "sqlite",
                    busytok_domain::LogSourceType::Directory => "directory",
                }
                .to_string(),
                root_path: source.root_path.display().to_string(),
                configured_by_user: source.configured_by_user as i32,
                default_discovery_enabled: 1,
                status: "active".to_string(),
                last_scan_started_at_ms: None,
                last_scan_completed_at_ms: Some(completed_ms),
                last_error: None,
                first_seen_at_ms: 0,
                last_seen_at_ms: completed_ms,
                created_at_ms: 0,
                updated_at_ms: completed_ms,
            };
            self.writer_handle
                .send(writer::WriteCommand::LogSourceUpsert(
                    writer::LogSourceUpsertCommand {
                        row: completion_row,
                    },
                ))
                .await
                .map_err(|e| anyhow::anyhow!("writer channel send error: {e}"))?;
        }

        // Persist the active generation in the supervisor so the tailer uses
        // the correct generation_id for events ingested after this point.
        self.generation_manager
            .activate_generation(gen_id.clone())?;

        info!(
            sources = sources.len(),
            files = total_files,
            generation_id = %gen_id,
            "new install sources registered for tailing"
        );

        Ok(ScanStats {
            sources: sources.len(),
            files_scanned: total_files,
            events_found: 0,
            diagnostics_found: 0,
        })
    }

    pub async fn run_initial_scan(&self) -> Result<ScanStats> {
        let sources = self.discover_sources()?;
        let preserves_real_degraded_state = {
            let readiness = self.status.read().await.readiness;
            readiness == ReadinessStateDto::ReadyDegraded
                && self.generation_manager.has_active_degradation_blocker()?
        };
        let existing_active_generation = if preserves_real_degraded_state {
            None
        } else {
            self.ensure_active_generation_for_existing_events()?
        };

        if preserves_real_degraded_state && sources.is_empty() {
            return Ok(ScanStats::default());
        }

        let (generation_id, needs_promotion) = match existing_active_generation {
            Some(gen_id) => (gen_id, false),
            None => {
                let gen_id = format!("gen-{}", busytok_domain::now_ms());
                self.writer_handle
                    .send(writer::WriteCommand::GenerationCreate(
                        writer::GenerationCreateCommand {
                            generation_id: gen_id.clone(),
                        },
                    ))
                    .await
                    .map_err(|e| anyhow::anyhow!("writer channel send error: {e}"))?;
                self.writer_handle
                    .flush()
                    .await
                    .context("failed to create initial scan generation")?;
                (gen_id, true)
            }
        };

        let db = {
            let db = self.db.lock().unwrap();
            db.reopen()?.ok_or_else(|| {
                anyhow::anyhow!("initial scan requires a detached database handle")
            })?
        };
        let adapters = self
            .adapters
            .lock()
            .unwrap()
            .iter()
            .map(|a| a.clone_boxed())
            .collect::<Vec<_>>();
        let timezone = self.settings.lock().unwrap().timezone.clone();
        let rtz = ReportingTimezone::parse(&timezone)?;

        let stats = if sources.is_empty() {
            ScanStats::default()
        } else {
            scan_once_via_writer(
                &db,
                &adapters,
                &sources,
                &self.event_bus,
                &rtz,
                &self.writer_handle,
                &generation_id,
            )
            .await?
        };

        if needs_promotion {
            self.writer_handle
                .send(writer::WriteCommand::PromotionBarrier(
                    writer::PromotionBarrierCommand {
                        from_generation_id: String::new(),
                        to_generation_id: generation_id.clone(),
                    },
                ))
                .await
                .map_err(|e| anyhow::anyhow!("writer channel send error: {e}"))?;
            self.writer_handle
                .flush()
                .await
                .context("failed to promote initial scan generation")?;
            self.generation_manager
                .activate_and_apply_ready_exact(generation_id.clone())?;
        } else {
            let promoted = self
                .mark_ready_exact_if_generation_valid(&generation_id)
                .await
                .context("failed to mark initial scan generation ready_exact")?;
            if !promoted {
                anyhow::bail!("initial scan active generation {generation_id} is not promoted");
            }
            self.generation_manager
                .activate_generation(generation_id.clone())?;
        }

        *self.last_scan_stats.lock().unwrap() = Some(stats.clone());
        Ok(stats)
    }

    /// Run the initial-scan code path with pre-discovered sources.
    ///
    /// This is the synchronous test-compat entry point. Production callers
    /// should use the async `run_initial_scan` instead.
    pub fn run_initial_scan_with_sources(
        &self,
        sources: Vec<busytok_discovery::DiscoveredLogSource>,
    ) -> Result<ScanStats> {
        self.run_detached_scan_for_sources(&sources)
    }

    /// Start live tailing of all discovered sources.
    ///
    /// Returns a `TailHandle` that can be used to shut down the tailer.
    /// The tailer shares the same database handle as the supervisor.
    pub async fn start_tailing(&self) -> Result<crate::tail::TailHandle> {
        let sources = self.discover_sources()?;
        let adapters = self
            .adapters
            .lock()
            .unwrap()
            .iter()
            .map(|a| a.clone_boxed())
            .collect();

        // Use a default generation ID when tailing without an active rebuild.
        // During a rebuild, the rebuild machinery (Task 7) will set an explicit
        // generation ID via the promotion barrier.
        let gen_id = self
            .generation_manager
            .active_generation_id()
            .unwrap_or_else(|| format!("gen-{}", busytok_domain::now_ms()));

        crate::tail::start_tailing(
            self.db.clone(),
            adapters,
            sources,
            self.event_bus.clone(),
            Arc::clone(&self.settings),
            self.writer_handle.clone(),
            gen_id,
        )
        .await
    }

    /// Run a scan using pre-discovered sources.
    ///
    /// Auto-creates an active generation if none exists. Production callers
    /// should prefer the async writer-actor path when a writer is available.
    pub fn run_scan_with_sources(
        &self,
        sources: Vec<busytok_discovery::DiscoveredLogSource>,
    ) -> Result<ScanStats> {
        let db = self.db.lock().unwrap();
        let adapters = self.adapters.lock().unwrap();
        let timezone = self.settings.lock().unwrap().timezone.clone();
        let rtz = ReportingTimezone::parse(&timezone)?;

        // Obtain or create a real generation so scanned data is visible
        // through the active-generation Overview read path.
        let gen_id = match self.generation_manager.active_generation_id() {
            Some(id) => id,
            None => {
                let new_id = format!("gen-{}", busytok_domain::now_ms());
                crate::rebuild::create_generation(&db, &new_id)?;
                self.generation_manager
                    .activate_generation(new_id.clone())?;
                new_id
            }
        };

        let stats = scan_once(&db, &adapters, &sources, &self.event_bus, &rtz, &gen_id)?;
        *self.last_scan_stats.lock().unwrap() = Some(stats.clone());
        Ok(stats)
    }

    /// Access the database handle (for testing and diagnostics).
    pub fn db_handle(&self) -> &Arc<std::sync::Mutex<Database>> {
        &self.db
    }

    /// Access the settings handle (for testing — e.g. injecting profile
    /// references to exercise `provider_delete`'s blocking check). Production
    /// code mutates settings via the RPC handlers (`provider_update`,
    /// `pi_sidecar_locator_update`, `profile_*`).
    pub fn settings_for_test(&self) -> Arc<Mutex<BusytokSettings>> {
        Arc::clone(&self.settings)
    }

    /// Access the resolved filesystem paths.
    ///
    /// Service shutdown uses this to remove the service.ready marker from
    /// `paths.data_dir()` at every exit point of `ServiceApp::run`.
    pub fn paths(&self) -> &BusytokPaths {
        &self.paths
    }

    /// Access the writer handle for enqueuing write commands.
    ///
    /// Callers (scanner, tailer, rebuilder) use this to send commands to the
    /// bounded writer actor instead of writing directly to the DB.
    pub fn writer_handle(&self) -> &WriterHandle {
        &self.writer_handle
    }

    /// Access the logical-subagent manager (for direct use outside the
    /// `RuntimeControl` impl). Returns an owned `Arc` clone because the
    /// manager lives behind `RwLock<SidecarRuntime>` — callers get a cheap
    /// Arc clone under a short read lock, then use it freely without holding
    /// the lock.
    pub fn subagent_manager(&self) -> Arc<busytok_subagent::SubagentManager> {
        self.sidecar_runtime
            .read()
            .unwrap()
            .subagent_manager
            .clone()
    }

    /// Phase 3 Task 5: bridge a subagent task's usage into the unified
    /// `usage_events` pipeline so it appears in Overview / Activity / receipts.
    ///
    /// This is the P0 + P1a fix: the runtime owns the `GenerationManager`
    /// (so the event gets the active `generation_id` — P0) and the rollup
    /// infrastructure (`build_scan_mutations` — P1a, produces REAL
    /// `daily_usage` + `model_summary` rows so the heatmap/receipt panels
    /// include subagent tokens). `RollupRows::default()` is forbidden here.
    ///
    /// Best-effort: the caller logs failures at `warn` but does NOT fail
    /// the task — the task result is already persisted; usage is
    /// best-effort observability.
    pub fn write_subagent_usage_event(
        &self,
        result: &busytok_subagent::models::DelegateResult,
        cwd: &str,
    ) -> Result<()> {
        let gen_id = self.generation_manager.active_generation_id();
        bridge_subagent_usage(&self.db, gen_id.as_deref(), &self.settings, result, cwd)
    }

    /// Gracefully drain and stop the writer actor.
    ///
    /// Service shutdown calls this after scan/tail tasks have stopped so the
    /// final metrics checkpoint is persisted before process exit.
    ///
    /// Task 7 Finding 3 fix: the §8.3 "queue only" background dispatcher is
    /// drained FIRST (send `true` on the watch + await the JoinHandle). The
    /// dispatcher writes directly to `db` (NOT through the writer actor), so
    /// it must finish before the writer actor's final flush + WAL checkpoint
    /// to avoid a race where the dispatcher commits a row after the writer
    /// has stopped accepting commands. No-op when no Tokio runtime was active
    /// at construction time (both fields are `None`).
    pub async fn shutdown_writer(&self) -> Result<()> {
        // 1. Drain the §8.3 task dispatcher (if running).
        if let Some(tx) = self.dispatcher_shutdown.lock().unwrap().take() {
            let _ = tx.send(true); // signal dispatcher to exit
        }
        if let Some(handle) = self.task_dispatcher.lock().unwrap().take() {
            let _ = handle.await; // wait for dispatcher to actually exit
        }

        // 2. Drain the writer actor.
        if self._writer_join.lock().unwrap().is_none() {
            return Ok(());
        }

        self.writer_handle.shutdown().await?;
        let join = self._writer_join.lock().unwrap().take();
        if let Some(join) = join {
            join.await
                .map_err(|e| anyhow::anyhow!("writer actor join error: {e}"))?;
        }
        Ok(())
    }

    /// Gracefully shut down the Pi sidecar subprocess(es) — hibernate
    /// sessions, kill children, reconcile DB state. Called from
    /// `ServiceApp::run()` after the control server has stopped accepting
    /// new delegate requests and before the tailer/sampler drain.
    ///
    /// Multi-provider fix: prefer `pool.shutdown_all()` to drain ALL
    /// workers (including lazily-spawned ones from delegate calls), not
    /// just the single "first enabled provider" supervisor. Without this,
    /// service exit would orphan Node subprocesses for any provider that
    /// was delegated to but wasn't the "first enabled" one. Falls back to
    /// single-supervisor shutdown only when no pool exists (degraded mode
    /// where construct_sidecar failed before pool creation). No-op when
    /// `pi_sidecar.enabled = false` (no pool, no supervisor). Failures are
    /// logged but do not propagate; service shutdown must continue so the
    /// writer actor flush and WAL checkpoint still run.
    pub async fn shutdown_sidecar(&self) {
        if let Some(pool) = self.worker_pool() {
            pool.shutdown_all().await;
        } else if let Some(sup) = self.sidecar_supervisor() {
            if let Err(e) = sup.shutdown().await {
                warn!(event_code = "subagent.sidecar.shutdown_failed", error = %e);
            }
        }
    }

    /// Return a clone of the event bus arc (for starting the sampler).
    pub fn event_bus_arc(&self) -> Arc<AppEventBus> {
        Arc::clone(&self.event_bus)
    }

    /// Return a clone of the status snapshot arc (for starting the sampler).
    pub fn status_snapshot_arc(&self) -> Arc<tokio::sync::RwLock<ServiceStatusSnapshot>> {
        Arc::clone(&self.status)
    }

    /// Detect legacy audit rows produced by known parser/token accounting bugs.
    ///
    /// This is intentionally non-destructive: callers can surface a warning or
    /// trigger a controlled rebuild path later, but startup should never wipe
    /// persisted audit state before a rebuild has proven it can succeed.
    pub fn legacy_audit_rebuild_recommended(&self) -> Result<bool> {
        let db = self.read_query_database()?;
        let conn = db.conn();

        let needs_codex_repair: i64 = conn.query_row(
            "SELECT COUNT(*) FROM usage_events WHERE agent = 'codex' AND (model IS NULL OR TRIM(model) = '')",
            [],
            |row| row.get(0),
        )?;
        let needs_claude_repair: i64 = conn.query_row(
            "SELECT COUNT(*) FROM usage_events \
             WHERE agent = 'claude_code' \
               AND total_tokens != (input_tokens + output_tokens + cache_creation_tokens + cache_read_tokens)",
            [],
            |row| row.get(0),
        )?;

        if needs_codex_repair == 0 && needs_claude_repair == 0 {
            return Ok(false);
        }

        warn!(
            codex_legacy_rows = needs_codex_repair,
            claude_legacy_rows = needs_claude_repair,
            "detected legacy audit rows; a controlled rebuild is recommended"
        );
        Ok(true)
    }

    /// Return the list of registered adapter agent names (for testing).
    pub fn debug_registered_agents(&self) -> Vec<String> {
        self.adapters
            .lock()
            .unwrap()
            .iter()
            .map(|a| a.agent().as_str().to_string())
            .collect()
    }

    fn scan_state_from_conn(conn: &Connection, service_running: bool) -> Result<&'static str> {
        let active_scan_threshold_ms = now_ms() - Self::ACTIVE_SCAN_GRACE_MS;
        let completed_sources: i64 = conn.query_row(
            "SELECT COUNT(*) FROM log_sources \
             WHERE status != 'removed' AND last_scan_completed_at_ms IS NOT NULL",
            [],
            |row| row.get(0),
        )?;
        let in_progress_sources: i64 = conn.query_row(
            "SELECT COUNT(*) FROM log_sources \
             WHERE status != 'removed' \
               AND last_scan_started_at_ms IS NOT NULL \
               AND last_scan_started_at_ms >= ?1 \
               AND (last_scan_completed_at_ms IS NULL OR last_scan_started_at_ms > last_scan_completed_at_ms)",
            [active_scan_threshold_ms],
            |row| row.get(0),
        )?;

        Ok(if !service_running {
            "offline"
        } else if in_progress_sources > 0 {
            "scanning"
        } else if completed_sources > 0 {
            "completed"
        } else {
            "idle"
        })
    }

    fn current_scan_state(&self) -> Result<&'static str> {
        let db = self.read_query_database()?;
        Self::scan_state_from_conn(
            db.conn(),
            busytok_config::service_marker::exists(self.paths.data_dir()),
        )
    }

    /// Apply a mutation to the in-memory service status snapshot.
    ///
    /// This is the primary API for other components (writer, scanner,
    /// rebuilder) to update the snapshot without holding the write lock
    /// for longer than necessary.
    pub fn apply_service_status_snapshot(
        &self,
        f: impl FnOnce(&mut ServiceStatusSnapshot),
    ) -> Result<()> {
        // Use try_write in sync context (called from non-async tests or
        // other synchronous paths).
        let mut snap = self
            .status
            .try_write()
            .map_err(|e| anyhow::anyhow!("status snapshot lock contention: {e}"))?;
        f(&mut snap);
        Ok(())
    }

    /// Read the current status snapshot (fast, in-memory).
    pub async fn read_status_snapshot(&self) -> ServiceStatusSnapshot {
        self.status.read().await.clone()
    }

    /// Hydrate the in-memory status snapshot from the persisted service_state
    /// row in the database.
    ///
    /// Called once during startup, before the control server is exposed, so
    /// `shell.status` returns meaningful readiness data immediately.
    pub fn hydrate_status_from_db(&self) -> Result<()> {
        let timezone = self.settings.lock().unwrap().timezone.clone();
        let gen_report = self
            .generation_manager
            .hydrate_from_db(&timezone)
            .context("failed to hydrate generation/readiness state")?;

        tracing::info!(
            readiness = ?gen_report.readiness,
            generation_id = ?gen_report.active_generation_id,
            latest_event_seq = ?gen_report.latest_event_seq,
            repaired = gen_report.repaired,
            "status snapshot hydrated from persisted service_state"
        );

        Ok(())
    }

    /// Transition readiness after initial scan completes.
    /// Delegates to [`GenerationManager`].
    pub async fn transition_after_initial_scan(
        &self,
        target_readiness: ReadinessStateDto,
    ) -> Result<bool> {
        self.generation_manager
            .transition_after_initial_scan(target_readiness)
            .await
    }
}

const PROMPT_LIST_DEFAULT_LIMIT: i64 = 100;

// ── Module-level helpers ─────────────────────────────────────────────

/// Bridge a subagent task's usage into the unified `usage_events` pipeline +
/// rollup tables. Extracted as a free function so BOTH the synchronous
/// `subagent_delegate()` path AND the background dispatcher (which runs
/// queued tasks) flow through the SAME seam — without this, queued tasks'
/// usage is invisible in Overview / Activity / receipt reads (P1 #2 fix).
///
/// `db` is the shared DB, `active_generation_id` is the current generation
/// (events written with any other generation_id are invisible to read paths),
/// `settings` provides the reporting timezone.
///
/// Best-effort: the caller logs failures at `warn` but does NOT fail the
/// task — the task result is already persisted; usage is best-effort
/// observability.
pub fn bridge_subagent_usage(
    db: &Arc<Mutex<Database>>,
    active_generation_id: Option<&str>,
    settings: &Arc<Mutex<BusytokSettings>>,
    result: &busytok_subagent::models::DelegateResult,
    cwd: &str,
) -> Result<()> {
    use busytok_aggregator::{
        build_scan_mutations, model_rollups_to_rows, project_rollups_to_rows,
        session_rollups_to_rows, RollupOptions,
    };
    use busytok_domain::UsageWritePolicy;
    use busytok_store::StoreWriteBatch;

    // P0: resolve the active generation_id. Events written with any other
    // generation_id are invisible to Overview/Activity read paths.
    let generation_id = active_generation_id
        .ok_or_else(|| anyhow::anyhow!("no active generation — cannot bridge subagent usage"))?;

    // Load the global PriceCatalog snapshot (same accessor scan/tail paths use).
    let catalog = busytok_pricing::load_catalog();

    let event = crate::subagent_usage::normalize_task_usage(
        &result.task_id,
        &result.subagent_id,
        cwd,
        &result.usage,
        Some(&catalog),
    );

    let input_tokens = event.input_tokens;
    let output_tokens = event.output_tokens;
    let model = event.model.clone();
    let cost_usd = event.cost_usd;

    let batch = StoreWriteBatch::for_test("subagent", &result.task_id)
        .usage_event(event, UsageWritePolicy::InsertOnce);

    // P1a: build REAL rollup rows via build_scan_mutations (NOT
    // RollupRows::default()). This mirrors tail.rs:570-581 — same closure
    // shape, same build_scan_mutations call, same RollupRows assembly.
    let timezone = settings.lock().unwrap().timezone.clone();
    let rtz = busytok_domain::ReportingTimezone::parse(&timezone).unwrap_or_else(|e| {
        tracing::warn!(
            event_code = "subagent.usage_tz_fallback",
            timezone = %timezone,
            error = %e,
            "timezone parse failed, falling back to UTC for subagent usage rollup"
        );
        busytok_domain::ReportingTimezone::utc()
    });
    let ro = RollupOptions {
        timezone: rtz.clone(),
    };

    let db_guard = db.lock().expect("subagent db lock poisoned");
    db_guard.ingest_store_batch(batch, &generation_id, |effective_events, gen_id| {
        if effective_events.is_empty() {
            return Ok(busytok_store::RollupRows::default());
        }
        let mutations = build_scan_mutations(effective_events, ro.clone(), gen_id)
            .context("failed to build subagent usage rollup mutations")?;
        Ok(busytok_store::RollupRows {
            daily_usage_rows: mutations.daily_usage,
            model_usage_rows: Vec::new(),
            session_rows: session_rollups_to_rows(&mutations.session_rollups),
            project_rows: project_rollups_to_rows(&mutations.project_rollups),
            model_summary_rows: model_rollups_to_rows(&mutations.model_rollups),
        })
    })?;

    tracing::info!(
        event_code = "subagent.usage_recorded",
        task_id = %result.task_id,
        model = ?model,
        input_tokens,
        output_tokens,
        cost_usd = ?cost_usd,
        generation_id = %generation_id,
        "recorded subagent usage in unified usage_events"
    );
    Ok(())
}

fn to_store_exact_windows(
    windows: &[range::TrendBucketWindow],
) -> Vec<busytok_store::read_models::OverviewExactWindow> {
    windows
        .iter()
        .map(|window| busytok_store::read_models::OverviewExactWindow {
            key: window.key.clone(),
            start_ms: window.start_ms,
            end_ms: window.end_ms,
        })
        .collect()
}

fn aggregate_trend_bucket(
    bucket: &range::TrendBucketWindow,
    granularity: &TrendBucketGranularityDto,
    rows: &[busytok_store::read_models::OverviewTrendBucketRow],
) -> OverviewTrendBucketDto {
    let mut tokens = 0;
    let mut cost_total = 0.0;
    let mut has_cost = false;
    let mut has_no_cost = false;
    let mut event_count = 0;

    for row in rows
        .iter()
        .filter(|row| row.start_ms >= bucket.start_ms && row.start_ms < bucket.end_ms)
    {
        tokens += row.tokens;
        event_count += row.event_count;
        has_cost |= row.has_cost;
        has_no_cost |= row.has_no_cost;
        if let Some(cost) = row.cost_usd {
            cost_total += cost;
        }
    }

    OverviewTrendBucketDto {
        key: bucket.key.clone(),
        label: ui_models::format_trend_label(granularity, &bucket.key),
        start_ms: bucket.start_ms,
        end_ms: bucket.end_ms,
        tokens,
        cost_usd: if has_cost { Some(cost_total) } else { None },
        cost_status: ui_models::cost_status(has_cost, has_no_cost),
        event_count,
        is_current: bucket.is_current,
    }
}

fn prompt_action_to_row(action: PromptActionDto) -> busytok_store::PromptActionRow {
    match action {
        PromptActionDto::OnlyCopy => busytok_store::PromptActionRow::Copy,
        PromptActionDto::OnlyPaste | PromptActionDto::CopyAndPaste => {
            busytok_store::PromptActionRow::Paste
        }
    }
}

fn prompt_sort_to_row(sort: Option<PromptSortDto>) -> busytok_store::PromptSortRow {
    match sort.unwrap_or(PromptSortDto::Smart) {
        PromptSortDto::Smart => busytok_store::PromptSortRow::Smart,
        PromptSortDto::RecentlyUsed => busytok_store::PromptSortRow::RecentlyUsed,
        PromptSortDto::MostUsed => busytok_store::PromptSortRow::MostUsed,
        PromptSortDto::RecentlyUpdated => busytok_store::PromptSortRow::RecentlyUpdated,
        PromptSortDto::Alphabetical => busytok_store::PromptSortRow::Alphabetical,
        PromptSortDto::PinnedFirst => busytok_store::PromptSortRow::PinnedFirst,
    }
}

fn prompt_use_surface_to_row(surface: PromptUseSurfaceDto) -> busytok_store::PromptUseSurfaceRow {
    match surface {
        PromptUseSurfaceDto::Overlay => busytok_store::PromptUseSurfaceRow::Overlay,
        PromptUseSurfaceDto::Page => busytok_store::PromptUseSurfaceRow::Page,
    }
}

fn prompt_use_outcome_to_row(outcome: PromptUseOutcomeDto) -> busytok_store::PromptUseOutcomeRow {
    match outcome {
        PromptUseOutcomeDto::Copy => busytok_store::PromptUseOutcomeRow::Copy,
        PromptUseOutcomeDto::PasteAttempted => busytok_store::PromptUseOutcomeRow::PasteAttempted,
        PromptUseOutcomeDto::PasteFellBackToCopy => {
            busytok_store::PromptUseOutcomeRow::PasteFellBackToCopy
        }
    }
}

fn prompt_use_failure_reason_to_row(
    reason: PromptUseFailureReasonDto,
) -> busytok_store::PromptUseFailureReasonRow {
    match reason {
        PromptUseFailureReasonDto::PermissionMissing => {
            busytok_store::PromptUseFailureReasonRow::PermissionMissing
        }
        PromptUseFailureReasonDto::FocusLost => busytok_store::PromptUseFailureReasonRow::FocusLost,
        PromptUseFailureReasonDto::InjectionFailed => {
            busytok_store::PromptUseFailureReasonRow::InjectionFailed
        }
        PromptUseFailureReasonDto::UnsupportedPlatform => {
            busytok_store::PromptUseFailureReasonRow::UnsupportedPlatform
        }
    }
}

fn prompt_entry_to_dto(row: busytok_store::PromptEntryRow) -> PromptEntryDto {
    PromptEntryDto {
        id: row.id,
        content: row.content,
        alias: row.alias,
        tags: row.tags,
        is_pinned: row.is_pinned,
        usage_count: row.usage_count,
        last_used_at_ms: row.last_used_at_ms,
        created_at_ms: row.created_at_ms,
        updated_at_ms: row.updated_at_ms,
    }
}

fn prompt_list_query_to_row(req: PromptListQueryDto) -> busytok_store::PromptListQuery {
    busytok_store::PromptListQuery {
        query: req.query,
        tag: req.tag,
        sort: prompt_sort_to_row(req.sort),
        limit: req.limit.unwrap_or(PROMPT_LIST_DEFAULT_LIMIT),
    }
}

// ── Private helpers ──────────────────────────────────────────────────

impl BusytokSupervisor {
    /// Unified cache-hit rate for a list row, null on invariant violation.
    /// Reads the row's persisted unified fields directly — `from_raw` is
    /// ingest-only; the read path consumes stored fields and never re-derives
    /// the provider payload shape.
    fn list_cache_hit_rate(row: &busytok_store::read_models::ActivityListRow) -> Option<f64> {
        let m = busytok_domain::cache_metrics::UnifiedCacheMetrics {
            prompt_input_total_tokens: row.prompt_input_total_tokens,
            prompt_input_non_cached_tokens: row.prompt_input_non_cached_tokens,
            cache_read_tokens: row.cache_read_tokens,
            cache_write_tokens: row.cache_creation_tokens,
        };
        busytok_domain::cache_metrics::cache_hit_rate(m)
    }

    fn activity_item_from_read_row(
        item: &busytok_store::read_models::ActivityListRow,
    ) -> ActivityListItemDto {
        let cost_status = ui_models::cost_status(item.cost_usd.is_some(), item.cost_usd.is_none());
        let cache_hit_rate = Self::list_cache_hit_rate(item);
        ActivityListItemDto {
            id: item.id.clone(),
            happened_at_ms: item.happened_at_ms,
            client_id: item.client_kind.clone(),
            client_label: ui_models::client_label(&item.client_kind),
            source_id: None,
            source_label: None,
            source_root_path: Some(item.source_path.clone()),
            project_label: item.project_path.clone(),
            project_hash: item.project_hash.clone(),
            model_id: item.model.clone(),
            model_label: item.model.as_deref().map(ui_models::model_label),
            tokens: item.total_tokens,
            cache_hit_rate,
            cost_usd: ui_models::cost_usd_for_status(item.cost_usd, &cost_status),
            cost_status,
            status: ui_models::activity_status(item.is_error),
            detail_available: true,
        }
    }

    fn activity_detail_from_read_row(
        event: busytok_store::read_models::ActivityDetailRow,
        source_info: Option<(String, String, String)>,
    ) -> ActivityDetailDto {
        let cost_status =
            ui_models::cost_status(event.cost_usd.is_some(), event.cost_usd.is_none());
        let has_components = event.input_tokens > 0
            || event.output_tokens > 0
            || event.cached_input_tokens > 0
            || event.reasoning_tokens > 0
            || event.thoughts_tokens > 0
            || event.tool_tokens > 0
            || event.cache_creation_tokens > 0
            || event.cache_read_tokens > 0;

        let unified = busytok_domain::cache_metrics::UnifiedCacheMetrics {
            prompt_input_total_tokens: event.prompt_input_total_tokens,
            prompt_input_non_cached_tokens: event.prompt_input_non_cached_tokens,
            cache_read_tokens: event.cache_read_tokens,
            cache_write_tokens: event.cache_creation_tokens,
        };
        let detail_rate = busytok_domain::cache_metrics::cache_hit_rate(unified);
        let token_breakdown = has_components.then(|| TokenBreakdownDto {
            prompt_input_total_tokens: (event.prompt_input_total_tokens > 0)
                .then_some(event.prompt_input_total_tokens),
            prompt_input_non_cached_tokens: (event.prompt_input_non_cached_tokens > 0)
                .then_some(event.prompt_input_non_cached_tokens),
            cache_read_tokens: (event.cache_read_tokens > 0).then_some(event.cache_read_tokens),
            cache_write_tokens: (event.cache_creation_tokens > 0)
                .then_some(event.cache_creation_tokens),
            cache_hit_rate: detail_rate,
            input_tokens: (event.input_tokens > 0).then_some(event.input_tokens),
            output_tokens: (event.output_tokens > 0).then_some(event.output_tokens),
            cached_input_tokens: (event.cached_input_tokens > 0)
                .then_some(event.cached_input_tokens),
            reasoning_tokens: (event.reasoning_tokens > 0).then_some(event.reasoning_tokens),
            total_tokens: event.total_tokens,
        });

        let mut notes = Vec::new();
        if let Some(ref speed) = event.speed {
            if !speed.is_empty() {
                notes.push(format!("speed: {speed}"));
            }
        }
        if event.cache_creation_tokens > 0 {
            notes.push(format!(
                "cache_creation_tokens: {}",
                event.cache_creation_tokens
            ));
        }
        if event.cache_read_tokens > 0 {
            notes.push(format!("cache_read_tokens: {}", event.cache_read_tokens));
        }
        if event.thoughts_tokens > 0 {
            notes.push(format!("thoughts_tokens: {}", event.thoughts_tokens));
        }
        if event.tool_tokens > 0 {
            notes.push(format!("tool_tokens: {}", event.tool_tokens));
        }
        if let Some(reset_time) = event.usage_limit_reset_time_ms {
            notes.push(format!("usage_limit_reset_time_ms: {reset_time}"));
        }

        let model_display = event.model.as_deref().unwrap_or("");

        ActivityDetailDto {
            id: event.id.clone(),
            title: format!("{} tokens", event.total_tokens),
            subtitle: Some(format!("{} event", event.agent.as_str())),
            happened_at_ms: event.timestamp_ms,
            client_id: event.client_kind.clone().unwrap_or_default(),
            client_label: ui_models::client_label(event.client_kind.as_deref().unwrap_or("")),
            source_id: source_info.as_ref().map(|s| s.0.clone()),
            source_label: source_info.as_ref().map(|s| s.1.clone()),
            source_root_path: source_info.as_ref().map(|s| s.2.clone()),
            project_label: event.project_path.clone(),
            project_hash: event.project_hash.clone(),
            session_id: Some(event.session_id.clone()),
            model_id: event.model.clone(),
            model_label: if model_display.is_empty() {
                None
            } else {
                Some(ui_models::model_label(model_display))
            },
            cost_status,
            status: ui_models::activity_status(event.is_error),
            tokens: event.total_tokens,
            token_breakdown,
            cost_usd: ui_models::cost_usd_for_status(event.cost_usd, &cost_status),
            technical_details: ActivityTechnicalDetailsDto {
                source_id: source_info.as_ref().map(|s| s.0.clone()),
                provider: event.model_provider,
                raw_model: event.model,
                notes,
            },
        }
    }
}

impl BusytokSupervisor {
    fn timezone_and_weekday(&self) -> (String, WeekdayIndexDto) {
        let s = self.settings.lock().unwrap();
        (
            s.timezone.clone(),
            WeekdayIndexDto::from_u8(s.week_starts_on).unwrap_or(WeekdayIndexDto::MONDAY),
        )
    }

    fn readiness_label(readiness: ReadinessStateDto) -> &'static str {
        match readiness {
            ReadinessStateDto::Starting => "starting",
            ReadinessStateDto::Rebuilding => "rebuilding",
            ReadinessStateDto::ReadyDegraded => "ready_degraded",
            ReadinessStateDto::ReadyExact => "ready_exact",
        }
    }

    async fn active_generation_id_from_snapshot(&self) -> Result<String> {
        let snap = self.status.read().await;
        snap.active_generation_id
            .clone()
            .ok_or_else(|| anyhow::anyhow!("read model unavailable: no active generation"))
    }

    async fn read_query(
        &self,
        method: &str,
        query_family: &str,
        used_read_model: bool,
    ) -> crate::read_service::ReadQuery {
        let snap = self.status.read().await;
        crate::read_service::ReadQuery::new(method, query_family)
            .generation_id_opt(snap.active_generation_id.clone())
            .readiness_opt(Some(Self::readiness_label(snap.readiness).to_string()))
            .watermark_ms_opt(snap.latest_event_seq)
            .used_read_model(used_read_model)
    }

    async fn run_read_with_mode<T, R, F>(
        &self,
        method: &str,
        query_family: &str,
        used_read_model: bool,
        f: F,
    ) -> Result<T>
    where
        T: Send + 'static,
        R: Into<crate::read_service::ReadOutcome<T>> + Send + 'static,
        F: FnOnce(&rusqlite::Connection) -> Result<R> + Send + 'static,
    {
        let query = self.read_query(method, query_family, used_read_model).await;
        self.run_read(query, f).await
    }

    async fn run_read<T, R, F>(&self, query: crate::read_service::ReadQuery, f: F) -> Result<T>
    where
        T: Send + 'static,
        R: Into<crate::read_service::ReadOutcome<T>> + Send + 'static,
        F: FnOnce(&rusqlite::Connection) -> Result<R> + Send + 'static,
    {
        self.read_service.run(query, f).await.map_err(|err| {
            anyhow::Error::new(MethodDispatchError::from_read_error(
                err.code(),
                err.to_string(),
                serde_json::json!({
                    "code": err.code(),
                    "kind": format!("{:?}", err.kind()),
                    "method": err.method(),
                    "query_family": err.query_family(),
                    "message": err.message(),
                }),
            ))
        })
    }

    /// Like `run_read_with_mode`, but falls back to a synchronous readonly
    /// connection when no Tokio runtime is active (sync test contexts).
    async fn run_read_or_fallback<T, R, F>(
        &self,
        method: &str,
        query_family: &str,
        used_read_model: bool,
        f: F,
    ) -> Result<T>
    where
        T: Send + 'static,
        R: Into<crate::read_service::ReadOutcome<T>> + Send + 'static,
        F: FnOnce(&rusqlite::Connection) -> Result<R> + Send + 'static,
    {
        if tokio::runtime::Handle::try_current().is_err() {
            // No Tokio runtime — use a synchronous readonly connection.
            let db = self.read_query_database()?;
            return f(db.conn())
                .map(Into::into)
                .map(|outcome| outcome.value)
                .map_err(Into::into);
        }
        self.run_read_with_mode(method, query_family, used_read_model, f)
            .await
    }

    /// Build a `ReadEnvelopeDto<T>` from the in-memory status snapshot.
    ///
    /// Uses `try_read` (non-async) to avoid holding an `RwLockReadGuard` across
    /// an `.await` point, which would make the calling async fn's future `!Send`.
    /// All read-plane methods use this to populate readiness, generation_id,
    /// is_exact, is_stale, progress, and degraded_reason in a consistent way.
    fn build_read_envelope<T>(&self, data: T, generated_at_ms: i64) -> Result<ReadEnvelopeDto<T>> {
        let snap = self
            .status
            .try_read()
            .map_err(|e| anyhow::anyhow!("status snapshot lock contention: {e}"))?;
        let generation_id = snap.active_generation_id.clone();
        let readiness = snap.readiness;
        let is_exact = matches!(readiness, ReadinessStateDto::ReadyExact);
        let is_stale = matches!(
            readiness,
            ReadinessStateDto::Starting | ReadinessStateDto::ReadyDegraded
        );
        let degraded_reason = match readiness {
            ReadinessStateDto::ReadyDegraded => {
                Some("Read plane is operating in degraded mode".to_string())
            }
            _ => None,
        };
        Ok(ReadEnvelopeDto {
            data,
            generated_at_ms,
            generation_id,
            readiness,
            is_exact,
            is_stale,
            watermark_ms: snap.latest_event_seq,
            progress: snap.progress.clone(),
            degraded_reason,
        })
    }

    fn live_bucket_range(now_ms: i64, window_seconds: i64) -> (i64, i64) {
        let window_ms = window_seconds.max(2) * 1000;
        let end_ms = (now_ms / Self::LIVE_BUCKET_MS) * Self::LIVE_BUCKET_MS + Self::LIVE_BUCKET_MS;
        (end_ms - window_ms, end_ms)
    }

    fn densify_live_samples(
        start_ms: i64,
        end_ms: i64,
        sparse_samples: Vec<LiveSampleDto>,
    ) -> Vec<LiveSampleDto> {
        let mut by_bucket: BTreeMap<i64, LiveSampleDto> = sparse_samples
            .into_iter()
            .map(|sample| (sample.bucket_start_ms, sample))
            .collect();
        let mut samples = Vec::new();
        let mut cursor = start_ms;

        while cursor < end_ms {
            samples.push(by_bucket.remove(&cursor).unwrap_or(LiveSampleDto {
                bucket_start_ms: cursor,
                tokens_per_sec: 0.0,
                cost_per_sec: None,
                events_per_sec: 0.0,
            }));
            cursor += Self::LIVE_BUCKET_MS;
        }

        samples
    }
}

// ---------------------------------------------------------------------------
// Drop — best-effort shutdown of the §8.3 task dispatcher (Task 7 Finding 3
// fix). `Drop` cannot `.await`, so we can only send the shutdown signal; the
// dispatcher will exit on its next `select!` iteration (within ~200ms). Tests
// that need deterministic shutdown call `shutdown_writer()` (which awaits the
// JoinHandle) before letting the supervisor drop.
// ---------------------------------------------------------------------------

impl Drop for BusytokSupervisor {
    fn drop(&mut self) {
        // Send the shutdown signal if the sender is still owned (i.e. not
        // already taken by `shutdown_writer()`). Best-effort: ignore errors
        // (receiver already dropped).
        if let Some(tx) = self.dispatcher_shutdown.lock().unwrap().take() {
            let _ = tx.send(true);
        }
        // The JoinHandle is detached on drop (tokio semantics) — it does NOT
        // abort the task. The shutdown signal above guarantees the dispatcher
        // will exit within one poll cycle (200ms). We do NOT `take()` the
        // handle here so any subsequent `shutdown_writer()` call (rare, only
        // possible if drop is somehow re-entered) is a no-op.
    }
}

// ---------------------------------------------------------------------------
// Subagent bridge — conversion helpers between busytok-subagent models and
// busytok-protocol DTOs. Free functions (not `From` impls) because both ends
// are foreign types relative to this crate (avoids E0117 orphan rule).
// ---------------------------------------------------------------------------

fn map_subagent_error(e: busytok_subagent::SubagentError) -> anyhow::Error {
    MethodDispatchError::from_read_error(e.code(), e.to_string(), serde_json::Value::Null).into()
}

/// Validate the `runtime_dir` for `pi_sidecar_locator_update` (P1 contract
/// closure). Returns `Ok(())` when the path is a valid sidecar directory, or
/// a `MethodDispatchError` (code=`validation_error`) when not. Called BEFORE
/// the settings are persisted so a bad path never reaches `settings.toml`.
///
/// Reuses the same infrastructure as the doctor checks:
/// - `SidecarManifest` typed parse (manifest.rs) — catches malformed content,
///   not just absence (doctor check #5 `bundle_manifest_readable`).
/// - `node/<ARCH>/node` arch-aware path — mirrors doctor check #4
///   `bundled_node_arch` (supervisor.rs `bundled_node_arch` probe).
///
/// Checks (in order):
/// 1. Path is absolute (relative paths break the service-owned settings
///    contract — the GUI always sends absolute app-bundle paths).
/// 2. Directory exists (catches stale paths from uninstalled bundles).
/// 3. Contains `pi-sidecar.bundle.js` (minimum viable bundle — conventional
///    name; the manifest-referenced bundle is checked separately at step 6).
/// 4. Contains `manifest.json` (existence — content parsed at step 5).
/// 5. `manifest.json` parses as a valid `SidecarManifest` (catches malformed
///    content that would only fail later during doctor/delegate).
/// 6. The bundle filename declared in the manifest exists in the directory
///    (catches manifest/bundle drift — manifest says "foo.bundle.js" but only
///    "pi-sidecar.bundle.js" is on disk).
/// 7. When `node_runtime == "bundled"`: `node/<current-arch>/node` exists
///    (the runtime needs a real Node binary to spawn — without it,
///    `resolve_base_sidecar_config` would fail at rebuild time, entering
///    degraded `FailingTaskExecutor` mode. Catching this at validation
///    surfaces the error to the GUI immediately, before the rebuild).
fn validate_runtime_dir(runtime_dir: &str, node_runtime: &str) -> Result<()> {
    let path = Path::new(runtime_dir);
    if !path.is_absolute() {
        return Err(anyhow::Error::new(MethodDispatchError::from_read_error(
            "validation_error",
            "runtime_dir must be absolute".to_string(),
            serde_json::json!({ "field": "runtime_dir", "value": runtime_dir }),
        )));
    }
    if !path.is_dir() {
        return Err(anyhow::Error::new(MethodDispatchError::from_read_error(
            "validation_error",
            format!("runtime_dir does not exist or is not a directory: {runtime_dir}"),
            serde_json::json!({ "field": "runtime_dir", "value": runtime_dir }),
        )));
    }
    let bundle = path.join("pi-sidecar.bundle.js");
    if !bundle.exists() {
        return Err(anyhow::Error::new(MethodDispatchError::from_read_error(
            "validation_error",
            "runtime_dir missing required file: pi-sidecar.bundle.js".to_string(),
            serde_json::json!({
                "field": "runtime_dir",
                "missing": "pi-sidecar.bundle.js",
                "path": runtime_dir,
            }),
        )));
    }
    let manifest_path = path.join("manifest.json");
    if !manifest_path.exists() {
        return Err(anyhow::Error::new(MethodDispatchError::from_read_error(
            "validation_error",
            "runtime_dir missing required file: manifest.json".to_string(),
            serde_json::json!({
                "field": "runtime_dir",
                "missing": "manifest.json",
                "path": runtime_dir,
            }),
        )));
    }
    // Parse manifest.json content as a typed SidecarManifest. Catches
    // malformed JSON / missing required fields that the doctor's
    // `bundle_manifest_readable` check would flag — but BEFORE persisting.
    let manifest_contents = std::fs::read_to_string(&manifest_path).map_err(|e| {
        anyhow::Error::new(MethodDispatchError::from_read_error(
            "validation_error",
            format!("manifest.json not readable at {runtime_dir}: {e}"),
            serde_json::json!({
                "field": "runtime_dir",
                "missing": "manifest.json (readable)",
                "path": runtime_dir,
            }),
        ))
    })?;
    let manifest: busytok_config::SidecarManifest = serde_json::from_str(&manifest_contents)
        .map_err(|e| {
            anyhow::Error::new(MethodDispatchError::from_read_error(
                "validation_error",
                format!("manifest.json is not a valid SidecarManifest: {e}"),
                serde_json::json!({
                    "field": "manifest.json",
                    "error": e.to_string(),
                    "path": runtime_dir,
                }),
            ))
        })?;
    // Verify the bundle filename declared in the manifest actually exists.
    // Catches manifest/bundle drift (manifest says "foo.bundle.js" but the
    // actual file is named differently).
    let manifest_bundle = path.join(&manifest.bundle);
    if !manifest_bundle.exists() {
        return Err(anyhow::Error::new(MethodDispatchError::from_read_error(
            "validation_error",
            format!(
                "manifest references bundle '{}' but file not found at {}",
                manifest.bundle, runtime_dir
            ),
            serde_json::json!({
                "field": "manifest.bundle",
                "expected": manifest.bundle,
                "path": runtime_dir,
            }),
        )));
    }
    // When using bundled Node, verify the current-arch binary exists AND is
    // executable. The doctor's `bundled_node_arch` check also does the
    // existence half, but catching it at validation surfaces the error to the
    // GUI before the rebuild enters degraded mode. System mode doesn't need
    // the bundled binary. On Unix we additionally require any execute bit
    // (0o111) — a non-executable file would pass existence but fail at spawn.
    if node_runtime == "bundled" {
        let node_path = path.join("node").join(std::env::consts::ARCH).join("node");
        if !node_path.exists() {
            return Err(anyhow::Error::new(MethodDispatchError::from_read_error(
                "validation_error",
                format!(
                    "node_runtime='bundled' but node/{arch}/node not found at {runtime_dir}",
                    arch = std::env::consts::ARCH
                ),
                serde_json::json!({
                    "field": "node",
                    "arch": std::env::consts::ARCH,
                    "path": runtime_dir,
                }),
            )));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::metadata(&node_path)
                .map_err(|e| {
                    anyhow::Error::new(MethodDispatchError::from_read_error(
                        "validation_error",
                        format!(
                            "node_runtime='bundled' but cannot read node/{arch}/node permissions at {runtime_dir}: {e}",
                            arch = std::env::consts::ARCH
                        ),
                        serde_json::json!({
                            "field": "node",
                            "arch": std::env::consts::ARCH,
                            "path": runtime_dir,
                        }),
                    ))
                })?;
            if perms.permissions().mode() & 0o111 == 0 {
                return Err(anyhow::Error::new(MethodDispatchError::from_read_error(
                    "validation_error",
                    format!(
                        "node_runtime='bundled' but node/{arch}/node is not executable at {runtime_dir}",
                        arch = std::env::consts::ARCH
                    ),
                    serde_json::json!({
                        "field": "node",
                        "arch": std::env::consts::ARCH,
                        "path": runtime_dir,
                    }),
                )));
            }
        }
    }
    Ok(())
}

fn delegate_request_from_dto(
    d: busytok_protocol::dto::SubagentDelegateRequestDto,
) -> busytok_subagent::models::DelegateRequest {
    busytok_subagent::models::DelegateRequest {
        subagent_name: d.subagent_name,
        subagent_id: d.subagent_id,
        cwd: d.cwd,
        profile: d.profile,
        intent: d.intent,
        prompt: d.prompt,
        prompt_artifact_ref: d.prompt_artifact_ref,
        timeout_seconds: d.timeout_seconds,
        model_override: d.model_override,
        source_harness: d.source_harness,
        source_session_id: d.source_session_id,
        // Spec §3.3: bound fields come from the DTO. For the reuse path
        // (existing subagent by name or subagent_id), the manager IGNORES
        // these and uses the subagent's stored bound fields. For the create
        // path, the manager enforces "both or neither".
        bound_provider_id: d.bound_provider_id,
        bound_model_id: d.bound_model_id,
    }
}

fn resolve_params_from_dto(
    r: busytok_protocol::dto::SubagentResolveRequestDto,
) -> busytok_subagent::models::ResolveParams {
    busytok_subagent::models::ResolveParams {
        name: r.name,
        id: r.id,
        cwd: r.cwd,
    }
}

fn subagent_detail(s: busytok_subagent::models::LogicalSubagent) -> SubagentDetailDto {
    SubagentDetailDto {
        id: s.id,
        name: s.name,
        project_id: s.project_id,
        repo_path: s.repo_path,
        repo_hash: s.repo_hash,
        branch: s.branch,
        intent: s.intent,
        default_profile: s.default_profile,
        // Spec §3.3: per-subagent bound fields (NOT NULL in store) replace
        // the former `default_model` field. Surfaced on the detail DTO so
        // the GUI/CLI can show which provider+model the subagent is bound to.
        bound_provider_id: s.bound_provider_id,
        bound_model_id: s.bound_model_id,
        status: s.status.as_str().to_string(),
        created_at_ms: s.created_at_ms,
        updated_at_ms: s.updated_at_ms,
        last_active_at_ms: s.last_active_at_ms,
    }
}

fn subagent_task_summary(
    t: busytok_subagent::models::SubagentTaskSummary,
) -> SubagentTaskSummaryDto {
    SubagentTaskSummaryDto {
        id: t.id,
        subagent_id: t.subagent_id,
        profile: t.profile,
        status: t.status.as_str().to_string(),
        prompt: t.prompt,
        result_summary: t.result_summary,
        error: t.error,
        created_at_ms: t.created_at_ms,
        completed_at_ms: t.completed_at_ms,
    }
}

#[async_trait]
impl RuntimeControl for BusytokSupervisor {
    // ── Service ──────────────────────────────────────────────────────

    async fn service_health(&self) -> Result<ServiceHealthDto> {
        let db_healthy = self
            .run_read_or_fallback("service.health", "service_health", false, |conn| {
                // Any successful query proves the DB is reachable.
                let _: Vec<String> = conn
                    .prepare("SELECT name FROM sqlite_master WHERE type='table' LIMIT 1")
                    .and_then(|mut stmt| {
                        stmt.query_map([], |row| row.get::<_, String>(0))
                            .map(|rows| rows.filter_map(|r| r.ok()).collect())
                    })
                    .unwrap_or_default();
                Ok::<_, anyhow::Error>(true)
            })
            .await
            .unwrap_or(false);
        let scan_state = self.current_scan_state()?;
        Ok(ServiceHealthDto {
            ready: db_healthy && busytok_config::service_marker::exists(self.paths.data_dir()),
            db_healthy,
            scan_state: scan_state.to_string(),
        })
    }

    async fn service_status(&self) -> Result<ServiceStatusDto> {
        Ok(ServiceStatusDto {
            version: env!("CARGO_PKG_VERSION").to_string(),
            db_path: self.paths.db_path().display().to_string(),
            state: if busytok_config::service_marker::exists(self.paths.data_dir()) {
                "running".to_string()
            } else {
                "offline".to_string()
            },
        })
    }

    // ── Shell ────────────────────────────────────────────────────────

    async fn shell_status(&self) -> Result<ShellStatusDto> {
        let now_ms = busytok_domain::now_ms();

        // Read from in-memory snapshot for all counter and observability fields.
        let snap = self.status.read().await;
        let mut total_events = snap.total_usage_event_count;
        let mut source_count = snap.source_count;
        let chip_data_hydrated = snap.chip_data_hydrated;
        let mut client_rollups = snap.cached_client_rollups.clone();

        let readiness = snap.readiness;
        let latest_event_seq = snap.latest_event_seq;
        let writer_queue_depth = Some(snap.writer_queue_depth);
        let aggregate_lag_ms = Some(snap.aggregate_lag_ms);
        let subscription_bridge_connectivity = snap.subscription_bridge_connectivity.clone();
        let active_generation_id = snap.active_generation_id.clone();
        drop(snap);

        if !chip_data_hydrated {
            let active_gen = active_generation_id.clone();
            let counts_and_rollups = self
                .run_read_or_fallback(
                    "shell.status",
                    "shell_status_chip_hydration",
                    false,
                    move |conn| {
                        let total: i64 = conn
                            .query_row("SELECT COUNT(*) FROM usage_events", [], |r| r.get(0))
                            .unwrap_or(0);
                        let sources: i64 = conn
                            .query_row(
                                "SELECT COUNT(*) FROM log_sources WHERE status != 'removed'",
                                [],
                                |r| r.get(0),
                            )
                            .unwrap_or(0);
                        let rollups = active_gen
                            .as_deref()
                            .map(|gid| busytok_store::read_queries::read_client_rollups(conn, gid))
                            .transpose()
                            .unwrap_or_default()
                            .unwrap_or_default();
                        Ok::<_, anyhow::Error>((total, sources, rollups))
                    },
                )
                .await?;
            total_events = counts_and_rollups.0;
            source_count = counts_and_rollups.1;
            let hydrated_rollups: Vec<_> =
                counts_and_rollups.2.into_iter().map(|r| r.into()).collect();
            if let Ok(mut snap) = self.status.try_write() {
                snap.total_usage_event_count = total_events;
                snap.source_count = source_count;
                snap.cached_client_rollups = hydrated_rollups;
                snap.chip_data_hydrated = true;
            }
            // Re-read rollups from hydration result for chip generation below.
            client_rollups = self
                .status
                .try_read()
                .map(|s| s.cached_client_rollups.clone())
                .unwrap_or_default();
        }

        // Scan state: use cached value if fresh (< 10s), otherwise query DB.
        let service_running = busytok_config::service_marker::exists(self.paths.data_dir());
        let scan_state = {
            let snap = self.status.try_read().ok();
            let cache_hit = snap.and_then(|s| {
                match (s.cached_scan_state.as_ref(), s.scan_state_cached_at_ms) {
                    (Some(state), Some(cached_at))
                        if busytok_domain::now_ms() - cached_at < 10_000 =>
                    {
                        Some(state.clone())
                    }
                    _ => None,
                }
            });
            match cache_hit {
                Some(state) => state,
                None => {
                    let state = self
                        .run_read_or_fallback(
                            "shell.status",
                            "shell_status_scan_state",
                            false,
                            move |conn| {
                                Ok::<_, anyhow::Error>(
                                    Self::scan_state_from_conn(conn, service_running)?.to_string(),
                                )
                            },
                        )
                        .await?;
                    if let Ok(mut snap) = self.status.try_write() {
                        snap.cached_scan_state = Some(state.clone());
                        snap.scan_state_cached_at_ms = Some(busytok_domain::now_ms());
                    }
                    state
                }
            }
        };

        let mut status_chips = Vec::new();

        // Global scan-state chip remains first so service-level health is
        // always visible, even when we also show per-client activity chips.
        status_chips.push(StatusChipDto {
            id: "scan".to_string(),
            label: match scan_state.as_str() {
                "offline" => "Service offline".to_string(),
                "scanning" => "Scan in progress".to_string(),
                "completed" => "Live capture active".to_string(),
                _ => "Waiting for activity".to_string(),
            },
            tone: match scan_state.as_str() {
                "offline" => ToneDto::Danger,
                "completed" => ToneDto::Success,
                _ => ToneDto::Neutral,
            },
            detail: match scan_state.as_str() {
                "offline" => Some("Realtime capture is not running".to_string()),
                _ => None,
            },
            action: None,
        });

        // Per-client chips — one per discovered client.
        for rollup in &client_rollups {
            status_chips.push(StatusChipDto {
                id: format!("client:{}", rollup.client_kind),
                label: ui_models::client_label(&rollup.client_kind),
                tone: ui_models::client_rollup_tone(rollup.active_source_count),
                detail: Some(format!(
                    "{} sources, {} events",
                    rollup.active_source_count, rollup.event_count
                )),
                action: None,
            });
        }

        // Capture chip — has any data been collected?
        status_chips.push(StatusChipDto {
            id: "capture".to_string(),
            label: if total_events > 0 {
                format!("{} events captured", total_events)
            } else {
                "No data yet".to_string()
            },
            tone: if total_events > 0 {
                ToneDto::Success
            } else {
                ToneDto::Neutral
            },
            detail: Some(format!("{} sources", source_count)),
            action: None,
        });

        Ok(ShellStatusDto {
            generated_at_ms: now_ms,
            status_chips,
            readiness,
            latest_event_seq,
            writer_queue_depth,
            aggregate_lag_ms,
            subscription_bridge_connectivity,
        })
    }

    // ── Overview — modular ────────────────────────────────────────────

    async fn overview_summary(
        &self,
        req: OverviewSummaryRequestDto,
    ) -> Result<ReadEnvelopeDto<OverviewSummaryDto>> {
        let now_ms = busytok_domain::now_ms();
        let (timezone, week_starts_on) = self.timezone_and_weekday();

        let rtz = range::parse_timezone(&timezone).unwrap_or_else(|_| ReportingTimezone::utc());
        let (year, month, day) = rtz.today_civil_ymd().unwrap_or((2026, 1, 1));
        let r = range::resolve_range(&rtz, year, month, day, req.range, week_starts_on);
        let generation_id = self.active_generation_id_from_snapshot().await?;
        let range_window = busytok_store::read_models::RangeWindow::new(r.start_ms, r.end_ms);
        let use_fast_path = range::use_sql_fast_path(&rtz);

        let summary_generation_id = generation_id.clone();
        let summary_range = range_window.clone();
        let summary = self
            .run_read_with_mode("overview.summary", "overview_summary", true, move |conn| {
                if use_fast_path {
                    busytok_store::read_queries::read_overview_summary(
                        conn,
                        &summary_generation_id,
                        &summary_range,
                    )
                } else {
                    let tz_name = rtz.canonical_name();
                    busytok_store::read_queries::read_overview_summary_from_daily_usage(
                        conn,
                        tz_name,
                        &r.start_date,
                        &r.end_date,
                        &summary_generation_id,
                    )
                }
            })
            .await?;

        let cost_status = ui_models::cost_status(summary.has_cost, summary.has_no_cost);
        let totals = ui_models::UsageTotals::from(&summary);
        let metrics = ui_models::overview_metrics(req.range, &totals);

        self.build_read_envelope(
            OverviewSummaryDto {
                timezone,
                selected_range: req.range,
                cost_status,
                metrics,
                generated_at_ms: now_ms,
            },
            now_ms,
        )
    }

    // ── Receipt ──────────────────────────────────────────────────────

    async fn receipt_daily(
        &self,
        req: ReceiptDailyRequestDto,
    ) -> Result<ReadEnvelopeDto<ReceiptDailyDto>> {
        let now_ms = busytok_domain::now_ms();
        let (timezone, _week_starts_on) = self.timezone_and_weekday();
        let rtz = range::parse_timezone(&timezone).unwrap_or_else(|_| {
            warn!(event_code = "receipt.daily_tz_fallback", timezone = %timezone, "timezone parse failed, falling back to UTC");
            ReportingTimezone::utc()
        });
        let date = match req.date {
            Some(d) => d,
            None => rtz.local_date_for_timestamp_ms(now_ms).unwrap_or_else(|_| {
                warn!(
                    event_code = "receipt.daily_date_fallback",
                    "local date resolve failed, falling back to 1970-01-01 — receipt will be empty"
                );
                "1970-01-01".to_string()
            }),
        };
        let start_ms = rtz.civil_date_to_utc_start_ms(&date)?;
        let end_ms = rtz.civil_date_to_utc_start_ms(&rtz.next_civil_date(&date)?)?;
        let generation_id = self.active_generation_id_from_snapshot().await?;

        let tz_name = rtz.canonical_name().to_string();
        let date_for_closure = date.clone();
        let gen_for_closure = generation_id.clone();
        let data = self
            .run_read_with_mode("receipt.daily", "receipt_daily", true, move |conn| {
                let totals = busytok_store::read_queries::read_daily_receipt_totals(
                    conn,
                    &tz_name,
                    &date_for_closure,
                    &gen_for_closure,
                )?;
                let models = busytok_store::read_queries::read_daily_receipt_top_models(
                    conn,
                    &tz_name,
                    &date_for_closure,
                    &gen_for_closure,
                )?;
                let session_count = busytok_store::read_queries::read_session_count_for_window(
                    conn,
                    &gen_for_closure,
                    start_ms,
                    end_ms,
                )?;
                let peak_hour = busytok_store::read_queries::read_peak_hour_for_window(
                    conn,
                    &gen_for_closure,
                    start_ms,
                    end_ms,
                )?;
                Ok(crate::receipt::ReceiptDailyData {
                    totals,
                    models,
                    session_count,
                    peak_hour,
                })
            })
            .await?;

        let dto = crate::receipt::assemble_receipt_daily(data, &rtz, &date, now_ms)?;
        tracing::info!(
            event_code = "receipt.daily_served",
            date = %date,
            model_count = dto.top_models.len(),
            total_tokens = dto.metrics.total_tokens,
            "served daily receipt"
        );
        self.build_read_envelope(dto, now_ms)
    }

    async fn overview_trend(
        &self,
        req: OverviewTrendRequestDto,
    ) -> Result<ReadEnvelopeDto<OverviewTrendResponseDto>> {
        let now_ms = busytok_domain::now_ms();
        let (timezone, week_starts_on) = self.timezone_and_weekday();

        let rtz = range::parse_timezone(&timezone).unwrap_or_else(|_| ReportingTimezone::utc());
        let buckets = range::trend_buckets(&rtz, req.range, week_starts_on);
        let granularity = req
            .granularity
            .unwrap_or(ui_models::trend_granularity(req.range));
        let generation_id = self.active_generation_id_from_snapshot().await?;
        let use_fast_path = range::use_sql_fast_path(&rtz);
        let is_iana_day = !use_fast_path && matches!(req.range, RangePresetDto::Day);

        let trend_dto_buckets: Vec<OverviewTrendBucketDto> = if is_iana_day {
            // IANA Day: 24 hourly buckets via exact-window path.
            let exact_windows = to_store_exact_windows(&buckets);
            let trend_generation_id = generation_id;
            let rows = self
                .run_read_with_mode("overview.trend", "overview_trend", true, move |conn| {
                    busytok_store::read_queries::read_overview_window_aggregates_exact(
                        conn,
                        &trend_generation_id,
                        &exact_windows,
                    )
                })
                .await?;
            buckets
                .iter()
                .map(|bucket| aggregate_trend_bucket(bucket, &granularity, &rows))
                .collect()
        } else if use_fast_path {
            let trend_generation_id = generation_id;
            let trend_range_start = buckets.first().map(|b| b.start_ms).unwrap_or(0);
            let trend_range_end = buckets.last().map(|b| b.end_ms).unwrap_or(0);
            let rows = self
                .run_read_with_mode("overview.trend", "overview_trend", true, move |conn| {
                    busytok_store::read_queries::read_overview_trend_hourly(
                        conn,
                        &trend_generation_id,
                        trend_range_start,
                        trend_range_end,
                    )
                })
                .await?;
            buckets
                .iter()
                .map(|bucket| aggregate_trend_bucket(bucket, &granularity, &rows))
                .collect()
        } else {
            // IANA week/month/year: read from daily_usage materialized table.
            let tz_name = rtz.canonical_name().to_string();
            // Derive the daily_usage date range from bucket timestamps rather
            // than bucket keys: keys are date strings for Week/Month presets
            // but year-month ("2026-06") for Year preset, and using those as
            // half-open date-range bounds silently drops late-December rows.
            let first_date = buckets
                .first()
                .map(|b| {
                    rtz.local_date_for_timestamp_ms(b.start_ms)
                        .unwrap_or_default()
                })
                .unwrap_or_else(|| "1970-01-01".to_string());
            // bucket.end_ms is the exclusive upper bound (start of next period);
            // back off by 1ms to land on the last day inside the range, since
            // the SQL filter uses inclusive `<=`.
            let last_date = buckets
                .last()
                .map(|b| {
                    rtz.local_date_for_timestamp_ms(b.end_ms.saturating_sub(1))
                        .unwrap_or_default()
                })
                .unwrap_or_else(|| "1970-01-01".to_string());
            let trend_gen = generation_id.clone();
            let daily_rows: Vec<busytok_store::read_models::DailyUsageTrendRow> = self
                .run_read_with_mode(
                    "overview.trend",
                    "overview_trend_daily",
                    true,
                    move |conn| {
                        busytok_store::read_queries::read_overview_trend_from_daily_usage(
                            conn,
                            &tz_name,
                            &first_date,
                            &last_date,
                            &trend_gen,
                        )
                    },
                )
                .await?;
            // Map daily_usage rows to trend buckets. Bucket keys are date
            // strings for Day/Week/Month presets ("2026-06-12") and year-month
            // strings for Year preset ("2026-06"). starts_with handles both:
            // a 10-char date only starts_with itself, while a 7-char month
            // prefix matches every day in that month.
            buckets
                .iter()
                .map(|bucket| {
                    let bucket_key = &bucket.key;
                    let mut tokens = 0;
                    let mut cost_total = 0.0;
                    let mut has_cost = false;
                    let mut has_no_cost = false;
                    let mut event_count = 0;
                    for row in daily_rows
                        .iter()
                        .filter(|r| r.date.as_str().starts_with(bucket_key.as_str()))
                    {
                        tokens += row.tokens;
                        event_count += row.event_count;
                        has_cost |= row.has_cost;
                        has_no_cost |= row.has_no_cost;
                        if let Some(cost) = row.cost_usd {
                            cost_total += cost;
                        }
                    }
                    OverviewTrendBucketDto {
                        key: bucket.key.clone(),
                        label: ui_models::format_trend_label(&granularity, &bucket.key),
                        start_ms: bucket.start_ms,
                        end_ms: bucket.end_ms,
                        tokens,
                        cost_usd: if has_cost { Some(cost_total) } else { None },
                        cost_status: ui_models::cost_status(has_cost, has_no_cost),
                        event_count,
                        is_current: bucket.is_current,
                    }
                })
                .collect()
        };

        let has_cost = trend_dto_buckets
            .iter()
            .any(|b| b.cost_usd.as_ref().is_some_and(|c| *c > 0.0));
        let has_no_cost = trend_dto_buckets
            .iter()
            .any(|b| b.tokens > 0 && b.cost_usd.is_none());
        let cost_status = ui_models::cost_status(has_cost, has_no_cost);

        let trend = OverviewTrendDto {
            range: req.range,
            bucket_granularity: granularity,
            metric_options: vec![MetricOptionDto::Tokens, MetricOptionDto::Cost],
            cost_status,
            buckets: trend_dto_buckets,
        };

        self.build_read_envelope(OverviewTrendResponseDto { trend }, now_ms)
    }

    async fn overview_heatmap(
        &self,
        req: OverviewHeatmapRequestDto,
    ) -> Result<ReadEnvelopeDto<OverviewHeatmapResponseDto>> {
        let now_ms = busytok_domain::now_ms();
        let (timezone, week_starts_on) = self.timezone_and_weekday();

        let rtz = range::parse_timezone(&timezone).unwrap_or_else(|_| ReportingTimezone::utc());
        let heatmap_day_windows = range::heatmap_days(&rtz);
        let generation_id = self.active_generation_id_from_snapshot().await?;
        let use_fast_path = range::use_sql_fast_path(&rtz);

        let heatmap = if use_fast_path {
            let heatmap_generation_id = generation_id;
            let heatmap_start = heatmap_day_windows
                .first()
                .map(|window| window.start_ms)
                .unwrap_or(0);
            let heatmap_end = heatmap_day_windows
                .last()
                .map(|window| window.end_ms)
                .unwrap_or(0);
            let heatmap_rows = self
                .run_read_with_mode("overview.heatmap", "overview_heatmap", true, move |conn| {
                    busytok_store::read_queries::read_overview_trend_hourly(
                        conn,
                        &heatmap_generation_id,
                        heatmap_start,
                        heatmap_end,
                    )
                })
                .await?;
            OverviewHeatmapDto {
                today: rtz.today_local_date().unwrap_or_default(),
                week_starts_on,
                days: heatmap_day_windows
                    .iter()
                    .map(|window| {
                        let mut tokens = 0;
                        let mut cost_total = 0.0;
                        let mut has_cost = false;
                        let mut has_no_cost = false;
                        let mut event_count = 0;
                        for row in heatmap_rows.iter().filter(|row| {
                            row.start_ms >= window.start_ms && row.start_ms < window.end_ms
                        }) {
                            tokens += row.tokens;
                            event_count += row.event_count;
                            has_cost |= row.has_cost;
                            has_no_cost |= row.has_no_cost;
                            if let Some(cost) = row.cost_usd {
                                cost_total += cost;
                            }
                        }
                        OverviewHeatmapDayDto {
                            date: window.date.clone(),
                            tokens,
                            cost_usd: if has_cost { Some(cost_total) } else { None },
                            cost_status: ui_models::cost_status(has_cost, has_no_cost),
                            event_count,
                        }
                    })
                    .collect(),
            }
        } else {
            // IANA timezone: read heatmap from daily_usage materialized table.
            let tz_name = rtz.canonical_name().to_string();
            let first_date = heatmap_day_windows
                .first()
                .map(|w| w.date.clone())
                .unwrap_or_else(|| "1970-01-01".to_string());
            let last_date = heatmap_day_windows
                .last()
                .map(|w| w.date.clone())
                .unwrap_or_else(|| "1970-01-01".to_string());
            let heatmap_gen = generation_id.clone();
            let daily_rows: Vec<busytok_store::read_models::DailyUsageTrendRow> = self
                .run_read_with_mode(
                    "overview.heatmap",
                    "overview_heatmap_daily",
                    true,
                    move |conn| {
                        busytok_store::read_queries::read_overview_trend_from_daily_usage(
                            conn,
                            &tz_name,
                            &first_date,
                            &last_date,
                            &heatmap_gen,
                        )
                    },
                )
                .await?;
            OverviewHeatmapDto {
                today: rtz.today_local_date().unwrap_or_default(),
                week_starts_on,
                days: heatmap_day_windows
                    .iter()
                    .map(|window| {
                        let mut tokens = 0;
                        let mut cost_total = 0.0;
                        let mut has_cost = false;
                        let mut has_no_cost = false;
                        let mut event_count = 0;
                        for row in daily_rows.iter().filter(|r| r.date == window.date) {
                            tokens += row.tokens;
                            event_count += row.event_count;
                            has_cost |= row.has_cost;
                            has_no_cost |= row.has_no_cost;
                            if let Some(cost) = row.cost_usd {
                                cost_total += cost;
                            }
                        }
                        OverviewHeatmapDayDto {
                            date: window.date.clone(),
                            tokens,
                            cost_usd: if has_cost { Some(cost_total) } else { None },
                            cost_status: ui_models::cost_status(has_cost, has_no_cost),
                            event_count,
                        }
                    })
                    .collect(),
            }
        };

        let _ = req; // range param not used yet — heatmap is always 12 months

        self.build_read_envelope(OverviewHeatmapResponseDto { heatmap }, now_ms)
    }

    async fn overview_rankings(
        &self,
        req: OverviewRankingsRequestDto,
    ) -> Result<ReadEnvelopeDto<OverviewRankingsResponseDto>> {
        let now_ms = busytok_domain::now_ms();
        let (timezone, week_starts_on) = self.timezone_and_weekday();

        let rtz = range::parse_timezone(&timezone).unwrap_or_else(|_| ReportingTimezone::utc());
        let (year, month, day) = rtz.today_civil_ymd().unwrap_or((2026, 1, 1));
        let r = range::resolve_range(&rtz, year, month, day, req.range, week_starts_on);
        let generation_id = self.active_generation_id_from_snapshot().await?;

        let cost_generation_id = generation_id.clone();
        let cost_start_ms = r.start_ms;
        let cost_end_ms = r.end_ms;
        let cost_rankings = self
            .run_read_with_mode(
                "overview.rankings",
                "overview_rankings",
                true,
                move |conn| {
                    busytok_store::read_queries::read_overview_rankings_models_by_cost(
                        conn,
                        &cost_generation_id,
                        cost_start_ms,
                        cost_end_ms,
                        5,
                    )
                },
            )
            .await?;
        let model_generation_id = generation_id;
        let model_start_ms = r.start_ms;
        let model_end_ms = r.end_ms;
        let model_rankings = self
            .run_read_with_mode(
                "overview.rankings",
                "overview_rankings",
                true,
                move |conn| {
                    busytok_store::read_queries::read_overview_rankings_models(
                        conn,
                        &model_generation_id,
                        model_start_ms,
                        model_end_ms,
                        5,
                    )
                },
            )
            .await?;

        let max_cost = cost_rankings
            .iter()
            .filter_map(|r| r.total_cost_usd)
            .fold(0.0f64, |a, c| a.max(c))
            .max(0.0001);
        let max_model_tokens = model_rankings
            .iter()
            .map(|r| r.total_tokens)
            .max()
            .unwrap_or(1)
            .max(1) as f64;

        let mut rankings = Vec::with_capacity(2);

        rankings.push(OverviewRankingSectionDto {
            id: "costs".to_string(),
            title: "Top Costs".to_string(),
            items: cost_rankings
                .iter()
                .map(|r| {
                    let cs = ui_models::cost_status(r.has_cost, r.has_no_cost);
                    OverviewRankingItemDto {
                        id: r.group_key.clone(),
                        label: ui_models::model_label(&r.group_key),
                        value: ui_models::format_cost(
                            ui_models::cost_usd_for_status(r.total_cost_usd, &cs),
                            &cs,
                        ),
                        helper: None,
                        bar_value: r.total_cost_usd.unwrap_or(0.0) / max_cost * 100.0,
                        action: Some(StatusActionDto::OpenActivity),
                    }
                })
                .collect(),
        });

        rankings.push(OverviewRankingSectionDto {
            id: "models".to_string(),
            title: "Top Models".to_string(),
            items: model_rankings
                .iter()
                .map(|r| OverviewRankingItemDto {
                    id: r.group_key.clone(),
                    label: ui_models::model_label(&r.group_key),
                    value: ui_models::format_tokens(r.total_tokens),
                    helper: None,
                    bar_value: r.total_tokens as f64 / max_model_tokens * 100.0,
                    action: Some(StatusActionDto::OpenActivity),
                })
                .collect(),
        });

        self.build_read_envelope(OverviewRankingsResponseDto { rankings }, now_ms)
    }

    // ── Activity ─────────────────────────────────────────────────────

    async fn activity_recent(
        &self,
        req: ActivityRecentRequestDto,
    ) -> Result<ReadEnvelopeDto<ActivityRecentResponseDto>> {
        let now_ms = busytok_domain::now_ms();
        let (timezone, week_starts_on) = self.timezone_and_weekday();

        let rtz = range::parse_timezone(&timezone).unwrap_or_else(|_| ReportingTimezone::utc());
        let (year, month, day) = rtz.today_civil_ymd().unwrap_or((2026, 1, 1));
        let r = range::resolve_range(&rtz, year, month, day, req.range, week_starts_on);
        let limit = req.limit.unwrap_or(10).clamp(1, 500);
        let generation_id = self.active_generation_id_from_snapshot().await?;
        let recent_generation_id = generation_id;
        let recent = self
            .run_read_with_mode("activity.recent", "activity_recent", false, move |conn| {
                busytok_store::read_queries::read_activity_recent(
                    conn,
                    &recent_generation_id,
                    r.start_ms,
                    r.end_ms,
                    limit,
                )
            })
            .await?;

        let recent_activity: Vec<ActivityListItemDto> = recent
            .iter()
            .map(Self::activity_item_from_read_row)
            .collect();

        self.build_read_envelope(ActivityRecentResponseDto { recent_activity }, now_ms)
    }

    async fn activity_list(
        &self,
        req: ActivityListRequestDto,
    ) -> Result<ReadEnvelopeDto<ActivityListResponseDto>> {
        let now_ms = busytok_domain::now_ms();
        let (timezone, week_starts_on) = self.timezone_and_weekday();

        let rtz = range::parse_timezone(&timezone).unwrap_or_else(|_| ReportingTimezone::utc());
        let (year, month, day) = rtz.today_civil_ymd().unwrap_or((2026, 1, 1));
        let range = range::resolve_range(&rtz, year, month, day, req.range, week_starts_on);
        let generation_id = self.active_generation_id_from_snapshot().await?;
        let cursor = req.cursor.as_deref();
        let limit = req.limit.unwrap_or(100).clamp(1, 500) as i64;
        let list_generation_id = generation_id.clone();
        let list_cursor = cursor.map(|s| s.to_string());
        let list_page: busytok_store::read_models::CursorPage<
            busytok_store::read_models::ActivityListRow,
        > = self
            .run_read_with_mode("activity.list", "activity_list", false, move |conn| {
                let page = busytok_store::read_queries::read_activity_list(
                    conn,
                    &list_generation_id,
                    range.start_ms,
                    range.end_ms,
                    limit,
                    list_cursor.as_deref(),
                )?;
                let row_count = page.items.len();
                Ok(crate::read_service::ReadOutcome::with_row_count(
                    page, row_count,
                ))
            })
            .await?;

        let items: Vec<ActivityListItemDto> = list_page
            .items
            .iter()
            .map(Self::activity_item_from_read_row)
            .collect();
        let summary_generation_id = generation_id;
        let summary_range =
            busytok_store::read_models::RangeWindow::new(range.start_ms, range.end_ms);
        let summary_totals = self
            .run_read_with_mode("activity.list", "activity_list", true, move |conn| {
                busytok_store::read_queries::read_overview_summary(
                    conn,
                    &summary_generation_id,
                    &summary_range,
                )
            })
            .await?;

        let summary = ActivityListSummaryDto {
            item_count: summary_totals.event_count,
            total_tokens: summary_totals.total_tokens,
            total_cost_usd: summary_totals.total_cost_usd,
            cost_status: ui_models::cost_status(
                summary_totals.has_cost,
                summary_totals.has_no_cost,
            ),
        };

        self.build_read_envelope(
            ActivityListResponseDto {
                generated_at_ms: now_ms,
                items,
                next_cursor: list_page.next_cursor,
                summary,
            },
            now_ms,
        )
    }

    async fn activity_detail(
        &self,
        req: ActivityDetailRequestDto,
    ) -> Result<ReadEnvelopeDto<ActivityDetailDto>> {
        let generation_id = self.active_generation_id_from_snapshot().await?;
        let event_id = req.id;
        let detail_generation_id = generation_id;
        let (event, source_info) = self
            .run_read_with_mode("activity.detail", "activity_detail", false, move |conn| {
                let event = busytok_store::read_queries::read_activity_detail(
                    conn,
                    &event_id,
                    &detail_generation_id,
                )?;
                let source_info = busytok_store::read_queries::read_activity_source_info(
                    conn,
                    &event.source_file_id,
                )?
                .map(|row| {
                    (
                        row.source_id,
                        ui_models::client_label(&row.agent),
                        row.root_path,
                    )
                });
                Ok((event, source_info))
            })
            .await?;

        self.build_read_envelope(
            Self::activity_detail_from_read_row(event, source_info),
            busytok_domain::now_ms(),
        )
    }

    // ── Breakdown ────────────────────────────────────────────────────

    async fn breakdown_list(
        &self,
        req: BreakdownListRequestDto,
    ) -> Result<ReadEnvelopeDto<BreakdownListResponseDto>> {
        let now_ms = busytok_domain::now_ms();
        let (timezone, week_starts_on) = self.timezone_and_weekday();

        let rtz = range::parse_timezone(&timezone).unwrap_or_else(|_| ReportingTimezone::utc());
        let (year, month, day) = rtz.today_civil_ymd().unwrap_or((2026, 1, 1));
        let range = range::resolve_range(&rtz, year, month, day, req.range, week_starts_on);
        let generation_id = self.active_generation_id_from_snapshot().await?;
        let dimension = match req.kind {
            BreakdownKindDto::Project => busytok_store::read_models::BreakdownDimension::Project,
            BreakdownKindDto::Model => busytok_store::read_models::BreakdownDimension::Model,
            BreakdownKindDto::Session => busytok_store::read_models::BreakdownDimension::Session,
        };
        let start_date = range.start_date.clone();
        let end_date = range.end_date.clone();
        let list_generation_id = generation_id.clone();
        let list_cursor = req.cursor.clone();
        let list_limit = i64::from(req.limit.unwrap_or(100));
        let (result, totals) = self
            .run_read_with_mode("breakdown.list", "breakdown_list", true, move |conn| {
                let page = busytok_store::read_queries::read_breakdown_list(
                    conn,
                    &list_generation_id,
                    dimension,
                    &start_date,
                    &end_date,
                    list_limit,
                    list_cursor,
                )?;
                let row_count = page.items.len();
                let totals = busytok_store::read_queries::read_breakdown_totals(
                    conn,
                    &generation_id,
                    dimension,
                    &start_date,
                    &end_date,
                )?;
                Ok(crate::read_service::ReadOutcome::with_row_count(
                    (page, totals),
                    row_count,
                ))
            })
            .await?;

        // Map items based on kind, using enrichment values.
        let items: Vec<BreakdownListItemDto> = result
            .items
            .iter()
            .map(|item| {
                let cs = ui_models::cost_status(item.has_cost, item.has_no_cost);
                let label = item.label.clone().unwrap_or_else(|| item.group_key.clone());
                let subtitle = item.subtitle.clone();

                match req.kind {
                    BreakdownKindDto::Project => {
                        let top_model = item
                            .extra_values
                            .first()
                            .and_then(|v| v.as_ref())
                            .map(|m| ui_models::model_label(m));
                        BreakdownListItemDto::Project(ProjectBreakdownListItemDto {
                            id: item.group_key.clone(),
                            project_hash: item.group_key.clone(),
                            label,
                            subtitle,
                            tokens: item.total_tokens,
                            cost_usd: item.total_cost_usd,
                            cost_status: cs,
                            event_count: item.event_count,
                            last_active_at_ms: item.last_active_at_ms,
                            top_model_label: top_model,
                        })
                    }
                    BreakdownKindDto::Model => {
                        let client_labels: Vec<String> = item
                            .extra_values
                            .first()
                            .and_then(|v| v.as_ref())
                            .map(|s| {
                                s.split(',')
                                    .filter(|x| !x.is_empty())
                                    .map(|c| ui_models::client_label(c.trim()))
                                    .collect()
                            })
                            .unwrap_or_default();
                        let top_project =
                            item.extra_values.get(1).and_then(|v| v.as_ref().cloned());
                        BreakdownListItemDto::Model(ModelBreakdownListItemDto {
                            id: item.group_key.clone(),
                            label,
                            subtitle,
                            tokens: item.total_tokens,
                            cost_usd: item.total_cost_usd,
                            cost_status: cs,
                            event_count: item.event_count,
                            last_active_at_ms: item.last_active_at_ms,
                            client_labels,
                            top_project_label: top_project,
                        })
                    }
                    BreakdownKindDto::Session => {
                        let client_label = item
                            .extra_values
                            .first()
                            .and_then(|v| v.as_ref())
                            .map(|s| ui_models::client_label(s))
                            .unwrap_or_default();
                        let project_label =
                            item.extra_values.get(1).and_then(|v| v.as_ref().cloned());
                        let project_hash =
                            item.extra_values.get(2).and_then(|v| v.as_ref().cloned());
                        BreakdownListItemDto::Session(SessionBreakdownListItemDto {
                            id: item.group_key.clone(),
                            label,
                            subtitle,
                            tokens: item.total_tokens,
                            cost_usd: item.total_cost_usd,
                            cost_status: cs,
                            event_count: item.event_count,
                            last_active_at_ms: item.last_active_at_ms,
                            client_label,
                            project_label,
                            project_hash,
                        })
                    }
                }
            })
            .collect();

        let summary = BreakdownListResponseSummaryDto {
            item_count: totals.grouped_count,
            total_tokens: totals.total_tokens,
            total_cost_usd: totals.total_cost_usd,
            total_cost_status: ui_models::cost_status(totals.has_cost, totals.has_no_cost),
        };

        self.build_read_envelope(
            BreakdownListResponseDto {
                generated_at_ms: now_ms,
                kind: req.kind,
                items,
                next_cursor: result.next_cursor,
                summary,
            },
            busytok_domain::now_ms(),
        )
    }

    async fn breakdown_detail(
        &self,
        req: BreakdownDetailRequestDto,
    ) -> Result<ReadEnvelopeDto<BreakdownDetailDto>> {
        let now_ms = busytok_domain::now_ms();
        let (timezone, week_starts_on) = self.timezone_and_weekday();

        let rtz = range::parse_timezone(&timezone).unwrap_or_else(|_| ReportingTimezone::utc());
        let (year, month, day) = rtz.today_civil_ymd().unwrap_or((2026, 1, 1));
        let range = range::resolve_range(&rtz, year, month, day, req.range, week_starts_on);
        let buckets = range::trend_buckets(&rtz, req.range, week_starts_on);
        let generation_id = self.active_generation_id_from_snapshot().await?;
        let start_date = range.start_date.clone();
        let end_date = range.end_date.clone();
        let exact_windows = to_store_exact_windows(&buckets);
        let req_kind = req.kind;
        let req_range = req.range;
        let req_id = req.id;

        let detail = self
            .run_read_with_mode("breakdown.detail", "breakdown_detail", false, move |conn| {
                match req_kind {
                    BreakdownKindDto::Project => {
                        let activity_rows =
                            busytok_store::read_queries::read_breakdown_activity_list(
                                conn,
                                &generation_id,
                                busytok_store::read_models::BreakdownFilterField::Project,
                                &req_id,
                                range.start_ms,
                                range.end_ms,
                                1000,
                            )?;
                        let recent_activity = activity_rows
                            .iter()
                            .map(Self::activity_item_from_read_row)
                            .collect::<Vec<_>>();
                        let total_tokens: i64 = recent_activity.iter().map(|e| e.tokens).sum();
                        let has_cost = recent_activity.iter().any(|e| e.cost_usd.is_some());
                        let has_no_cost = recent_activity
                            .iter()
                            .any(|e| matches!(e.cost_status, CostStatusDto::Unavailable));
                        let total_cost_usd = has_cost
                            .then(|| recent_activity.iter().filter_map(|e| e.cost_usd).sum());
                        let totals = ui_models::UsageTotals {
                            total_tokens,
                            total_cost_usd,
                            event_count: recent_activity.len() as i64,
                            has_cost,
                            has_no_cost,
                        };
                        let metrics = ui_models::breakdown_metrics(req_range, &totals);
                        let bucket_granularity = ui_models::trend_granularity(req_range);
                        let trend_rows =
                            busytok_store::read_queries::read_breakdown_window_aggregates_exact(
                                conn,
                                &generation_id,
                                busytok_store::read_models::BreakdownFilterField::Project,
                                &req_id,
                                &exact_windows,
                            )?;
                        let trend = OverviewTrendDto {
                            range: req_range,
                            bucket_granularity,
                            metric_options: vec![MetricOptionDto::Tokens, MetricOptionDto::Cost],
                            cost_status: ui_models::cost_status(has_cost, has_no_cost),
                            buckets: buckets
                                .iter()
                                .map(|bucket| {
                                    aggregate_trend_bucket(bucket, &bucket_granularity, &trend_rows)
                                })
                                .collect(),
                        };

                        let mut model_map: std::collections::HashMap<String, BreakdownMiniItemDto> =
                            std::collections::HashMap::new();
                        for evt in &recent_activity {
                            let model = evt.model_id.clone().unwrap_or_default();
                            let entry =
                                model_map
                                    .entry(model.clone())
                                    .or_insert(BreakdownMiniItemDto {
                                        id: model.clone(),
                                        label: ui_models::model_label(&model),
                                        tokens: 0,
                                        cost_usd: None,
                                        cost_status: CostStatusDto::Unavailable,
                                        event_count: 0,
                                    });
                            entry.tokens += evt.tokens;
                            entry.event_count += 1;
                            if let Some(cost) = evt.cost_usd {
                                entry.cost_usd = Some(entry.cost_usd.unwrap_or(0.0) + cost);
                            }
                        }
                        let model_mix = model_map.into_values().collect::<Vec<_>>();

                        let session_items = busytok_store::read_queries::read_project_top_sessions(
                            conn,
                            &generation_id,
                            &req_id,
                            &start_date,
                            &end_date,
                            8,
                        )?;
                        let sessions = session_items
                            .iter()
                            .map(|s| {
                                let cs = ui_models::cost_status(s.has_cost, s.has_no_cost);
                                let label = s.label.clone().unwrap_or_else(|| s.group_key.clone());
                                let client_label = s
                                    .extra_values
                                    .first()
                                    .and_then(|v| v.as_ref())
                                    .map(|k| ui_models::client_label(k))
                                    .unwrap_or_default();
                                let project_label =
                                    s.extra_values.get(1).and_then(|v| v.as_ref().cloned());
                                SessionBreakdownListItemDto {
                                    id: s.group_key.clone(),
                                    label,
                                    subtitle: None,
                                    tokens: s.total_tokens,
                                    cost_usd: s.total_cost_usd,
                                    cost_status: cs,
                                    event_count: s.event_count,
                                    last_active_at_ms: s.last_active_at_ms,
                                    client_label,
                                    project_label,
                                    project_hash: Some(req_id.clone()),
                                }
                            })
                            .collect::<Vec<_>>();

                        let project_path = recent_activity
                            .first()
                            .and_then(|e| e.source_root_path.clone());

                        Ok(BreakdownDetailDto::Project(ProjectBreakdownDetailDto {
                            id: req_id.clone(),
                            label: req_id.clone(),
                            project_hash: req_id.clone(),
                            project_path,
                            metrics,
                            trend,
                            model_mix,
                            sessions,
                            recent_activity: recent_activity.into_iter().take(10).collect(),
                            technical_details: vec![],
                        }))
                    }
                    BreakdownKindDto::Model => {
                        let activity_rows =
                            busytok_store::read_queries::read_breakdown_activity_list(
                                conn,
                                &generation_id,
                                busytok_store::read_models::BreakdownFilterField::Model,
                                &req_id,
                                range.start_ms,
                                range.end_ms,
                                1000,
                            )?;
                        let recent_activity = activity_rows
                            .iter()
                            .map(Self::activity_item_from_read_row)
                            .collect::<Vec<_>>();
                        let total_tokens: i64 = recent_activity.iter().map(|e| e.tokens).sum();
                        let has_cost = recent_activity.iter().any(|e| e.cost_usd.is_some());
                        let has_no_cost = recent_activity
                            .iter()
                            .any(|e| matches!(e.cost_status, CostStatusDto::Unavailable));
                        let total_cost_usd = has_cost
                            .then(|| recent_activity.iter().filter_map(|e| e.cost_usd).sum());
                        let totals = ui_models::UsageTotals {
                            total_tokens,
                            total_cost_usd,
                            event_count: recent_activity.len() as i64,
                            has_cost,
                            has_no_cost,
                        };
                        let metrics = ui_models::breakdown_metrics(req_range, &totals);
                        let bucket_granularity = ui_models::trend_granularity(req_range);
                        let trend_rows =
                            busytok_store::read_queries::read_breakdown_window_aggregates_exact(
                                conn,
                                &generation_id,
                                busytok_store::read_models::BreakdownFilterField::Model,
                                &req_id,
                                &exact_windows,
                            )?;
                        let trend = OverviewTrendDto {
                            range: req_range,
                            bucket_granularity,
                            metric_options: vec![MetricOptionDto::Tokens, MetricOptionDto::Cost],
                            cost_status: ui_models::cost_status(has_cost, has_no_cost),
                            buckets: buckets
                                .iter()
                                .map(|bucket| {
                                    aggregate_trend_bucket(bucket, &bucket_granularity, &trend_rows)
                                })
                                .collect(),
                        };
                        let token_breakdown_row =
                            busytok_store::read_queries::read_model_token_breakdown(
                                conn,
                                &generation_id,
                                &req_id,
                                range.start_ms,
                                range.end_ms,
                            )?;
                        let agg = busytok_domain::cache_metrics::UnifiedCacheMetrics {
                            prompt_input_total_tokens: token_breakdown_row
                                .prompt_input_total_tokens,
                            prompt_input_non_cached_tokens: token_breakdown_row
                                .prompt_input_non_cached_tokens,
                            cache_read_tokens: token_breakdown_row.cache_read_tokens,
                            cache_write_tokens: token_breakdown_row.cache_creation_tokens,
                        };
                        let token_breakdown = TokenBreakdownDto {
                            prompt_input_total_tokens: Some(
                                token_breakdown_row.prompt_input_total_tokens,
                            )
                            .filter(|&v| v > 0),
                            prompt_input_non_cached_tokens: Some(
                                token_breakdown_row.prompt_input_non_cached_tokens,
                            )
                            .filter(|&v| v > 0),
                            cache_read_tokens: Some(token_breakdown_row.cache_read_tokens)
                                .filter(|&v| v > 0),
                            cache_write_tokens: Some(token_breakdown_row.cache_creation_tokens)
                                .filter(|&v| v > 0),
                            cache_hit_rate: busytok_domain::cache_metrics::cache_hit_rate(agg),
                            input_tokens: Some(token_breakdown_row.input_tokens).filter(|&v| v > 0),
                            output_tokens: Some(token_breakdown_row.output_tokens)
                                .filter(|&v| v > 0),
                            cached_input_tokens: Some(token_breakdown_row.cached_input_tokens)
                                .filter(|&v| v > 0),
                            reasoning_tokens: Some(token_breakdown_row.reasoning_tokens)
                                .filter(|&v| v > 0),
                            total_tokens,
                        };

                        let mut client_map: std::collections::HashMap<
                            String,
                            BreakdownMiniItemDto,
                        > = std::collections::HashMap::new();
                        for evt in &recent_activity {
                            let entry = client_map.entry(evt.client_id.clone()).or_insert(
                                BreakdownMiniItemDto {
                                    id: evt.client_id.clone(),
                                    label: evt.client_label.clone(),
                                    tokens: 0,
                                    cost_usd: None,
                                    cost_status: CostStatusDto::Unavailable,
                                    event_count: 0,
                                },
                            );
                            entry.tokens += evt.tokens;
                            entry.event_count += 1;
                        }
                        let client_mix = client_map.into_values().collect::<Vec<_>>();

                        let mut proj_map: std::collections::HashMap<
                            String,
                            ProjectBreakdownListItemDto,
                        > = std::collections::HashMap::new();
                        for evt in &recent_activity {
                            let key = evt.project_hash.clone().unwrap_or_default();
                            let entry = proj_map.entry(key.clone()).or_insert(
                                ProjectBreakdownListItemDto {
                                    id: key.clone(),
                                    project_hash: key.clone(),
                                    label: evt.project_label.clone().unwrap_or_else(|| key.clone()),
                                    subtitle: None,
                                    tokens: 0,
                                    cost_usd: None,
                                    cost_status: CostStatusDto::Unavailable,
                                    event_count: 0,
                                    last_active_at_ms: Some(evt.happened_at_ms),
                                    top_model_label: None,
                                },
                            );
                            entry.tokens += evt.tokens;
                            entry.event_count += 1;
                            if evt.happened_at_ms > entry.last_active_at_ms.unwrap_or(0) {
                                entry.last_active_at_ms = Some(evt.happened_at_ms);
                            }
                        }
                        let project_mix = proj_map.into_values().collect::<Vec<_>>();

                        Ok(BreakdownDetailDto::Model(ModelBreakdownDetailDto {
                            id: req_id.clone(),
                            label: ui_models::model_label(&req_id),
                            metrics,
                            trend,
                            token_breakdown,
                            client_mix,
                            project_mix,
                            recent_activity: recent_activity.into_iter().take(10).collect(),
                            technical_details: vec![],
                        }))
                    }
                    BreakdownKindDto::Session => {
                        let activity_rows =
                            busytok_store::read_queries::read_breakdown_activity_list(
                                conn,
                                &generation_id,
                                busytok_store::read_models::BreakdownFilterField::Session,
                                &req_id,
                                range.start_ms,
                                range.end_ms,
                                1000,
                            )?;
                        let recent_activity = activity_rows
                            .iter()
                            .map(Self::activity_item_from_read_row)
                            .collect::<Vec<_>>();
                        let total_tokens: i64 = recent_activity.iter().map(|e| e.tokens).sum();
                        let has_cost = recent_activity.iter().any(|e| e.cost_usd.is_some());
                        let has_no_cost = recent_activity
                            .iter()
                            .any(|e| matches!(e.cost_status, CostStatusDto::Unavailable));
                        let total_cost_usd = has_cost
                            .then(|| recent_activity.iter().filter_map(|e| e.cost_usd).sum());
                        let totals = ui_models::UsageTotals {
                            total_tokens,
                            total_cost_usd,
                            event_count: recent_activity.len() as i64,
                            has_cost,
                            has_no_cost,
                        };
                        let metrics = ui_models::breakdown_metrics(req_range, &totals);

                        let mut timeline_events = recent_activity.clone();
                        timeline_events.sort_by_key(|e| e.happened_at_ms);
                        let timeline = timeline_events
                            .iter()
                            .map(|e| SessionTimelineItemDto {
                                id: e.id.clone(),
                                happened_at_ms: e.happened_at_ms,
                                label: format!("{} tokens", e.tokens),
                                tokens: e.tokens,
                                cost_usd: e.cost_usd,
                                cost_status: e.cost_status,
                                status: e.status,
                            })
                            .collect::<Vec<_>>();

                        let mut model_map: std::collections::HashMap<String, BreakdownMiniItemDto> =
                            std::collections::HashMap::new();
                        for evt in &recent_activity {
                            let model = evt.model_id.clone().unwrap_or_default();
                            let entry =
                                model_map
                                    .entry(model.clone())
                                    .or_insert(BreakdownMiniItemDto {
                                        id: model.clone(),
                                        label: evt
                                            .model_label
                                            .clone()
                                            .unwrap_or_else(|| model.clone()),
                                        tokens: 0,
                                        cost_usd: None,
                                        cost_status: CostStatusDto::Unavailable,
                                        event_count: 0,
                                    });
                            entry.tokens += evt.tokens;
                            entry.event_count += 1;
                        }
                        let models_used = model_map.into_values().collect::<Vec<_>>();
                        let client_label = recent_activity
                            .first()
                            .map(|e| e.client_label.clone())
                            .unwrap_or_default();
                        let project_label = recent_activity
                            .first()
                            .and_then(|e| e.project_label.clone());
                        let project_hash =
                            recent_activity.first().and_then(|e| e.project_hash.clone());
                        let source_context_rows =
                            busytok_store::read_queries::read_session_source_context(
                                conn,
                                &generation_id,
                                &req_id,
                                5,
                            )?;
                        let source_context = source_context_rows
                            .into_iter()
                            .map(|row| SourceContextItemDto {
                                source_id: row.source_id,
                                client_label: ui_models::client_label(&row.agent),
                                root_path: row.root_path,
                            })
                            .collect::<Vec<_>>();

                        Ok(BreakdownDetailDto::Session(SessionBreakdownDetailDto {
                            id: req_id.clone(),
                            label: format!("Session {}", &req_id[..req_id.len().min(8)]),
                            client_id: recent_activity
                                .first()
                                .map(|e| e.client_id.clone())
                                .unwrap_or_default(),
                            client_label,
                            project_label,
                            project_hash,
                            last_active_at_ms: recent_activity.first().map(|e| e.happened_at_ms),
                            metrics,
                            token_breakdown: {
                                let sums = activity_rows.iter().fold(
                                    (0i64, 0i64, 0i64, 0i64, 0i64),
                                    // total, non_cached, read, write, cached_input(raw)
                                    |(t, nc, r, w, ci), row| {
                                        (
                                            t + row.prompt_input_total_tokens,
                                            nc + row.prompt_input_non_cached_tokens,
                                            r + row.cache_read_tokens,
                                            w + row.cache_creation_tokens,
                                            ci + row.cached_input_tokens,
                                        )
                                    },
                                );
                                let agg = busytok_domain::cache_metrics::UnifiedCacheMetrics {
                                    prompt_input_total_tokens: sums.0,
                                    prompt_input_non_cached_tokens: sums.1,
                                    cache_read_tokens: sums.2,
                                    cache_write_tokens: sums.3,
                                };
                                TokenBreakdownDto {
                                    prompt_input_total_tokens: Some(sums.0).filter(|&v| v > 0),
                                    prompt_input_non_cached_tokens: Some(sums.1).filter(|&v| v > 0),
                                    cache_read_tokens: Some(sums.2).filter(|&v| v > 0),
                                    cache_write_tokens: Some(sums.3).filter(|&v| v > 0),
                                    cache_hit_rate: busytok_domain::cache_metrics::cache_hit_rate(
                                        agg,
                                    ),
                                    input_tokens: None,
                                    output_tokens: None,
                                    cached_input_tokens: Some(sums.4).filter(|&v| v > 0),
                                    reasoning_tokens: None,
                                    total_tokens,
                                }
                            },
                            timeline,
                            models_used,
                            source_context,
                            technical_details: vec![],
                        }))
                    }
                }
            })
            .await?;

        self.build_read_envelope(detail, now_ms)
    }

    // ── Clients ──────────────────────────────────────────────────────

    async fn clients_snapshot(
        &self,
        req: ClientsSnapshotRequestDto,
    ) -> Result<ReadEnvelopeDto<ClientsSnapshotDto>> {
        let now_ms = busytok_domain::now_ms();
        let generation_id = self.active_generation_id_from_snapshot().await?;
        let status_filter = match req.scan_state {
            Some(SourceScanStateDto::Error) => Some("error"),
            Some(SourceScanStateDto::Warning) => Some("warning"),
            Some(SourceScanStateDto::Scanning) => Some("scanning_or_active"),
            Some(SourceScanStateDto::Idle) => Some("idle"),
            None => None,
        };
        let source_limit = i64::from(req.limit.unwrap_or(100));
        let source_cursor = req.cursor.clone();
        let source_client_id = req.client_id.clone();
        let sources_generation_id = generation_id.clone();
        let rollups_generation_id = generation_id.clone();
        let (result, summary_row, rollups) = self
            .run_read_with_mode("clients.snapshot", "clients_snapshot", true, move |conn| {
                let page = busytok_store::read_queries::read_source_health_summaries(
                    conn,
                    &sources_generation_id,
                    source_limit,
                    source_cursor,
                    source_client_id.as_deref(),
                    status_filter,
                )?;
                let row_count = page.items.len();
                let summary = busytok_store::read_queries::read_source_health_summary_totals(
                    conn,
                    &sources_generation_id,
                    source_client_id.as_deref(),
                    status_filter,
                )?;
                let rollups =
                    busytok_store::read_queries::read_client_rollups(conn, &rollups_generation_id)?;
                Ok(crate::read_service::ReadOutcome::with_row_count(
                    (page, summary, rollups),
                    row_count,
                ))
            })
            .await?;

        let client_cards: Vec<ClientStatusCardDto> = rollups
            .into_iter()
            .map(|rollup| ClientStatusCardDto {
                id: rollup.client_kind.clone(),
                label: ui_models::client_label(&rollup.client_kind),
                tone: ui_models::client_rollup_tone(rollup.active_source_count),
                active_source_count: rollup.active_source_count,
                event_count: rollup.event_count,
                last_scan_at_ms: rollup.last_scan_at_ms,
                helper: None,
            })
            .collect();

        // Map sources to DTOs (no re-filtering needed — already done in SQL).
        let sources: Vec<ClientSourceRowDto> = result
            .items
            .iter()
            .map(|src| ClientSourceRowDto {
                id: src.source_id.clone(),
                client_id: src.agent.clone(),
                client_label: ui_models::client_label(&src.agent),
                root_path: src.root_path.clone(),
                source_type: match src.source_type.as_str() {
                    "manual" => SourceTypeDto::ManualRoot,
                    _ => SourceTypeDto::DefaultDiscovery,
                },
                scan_state: ui_models::source_scan_state(&src.status),
                configured_by_user: src.configured_by_user,
                last_scan_at_ms: src.last_scan_at_ms,
                file_count: src.file_count,
                parsed_file_count: src.parsed_file_count,
                event_count: src.event_count,
                last_error: src.last_error.clone(),
            })
            .collect();

        let summary = ClientsSnapshotSummaryDto {
            source_count: summary_row.source_count,
            active_source_count: summary_row.active_source_count,
        };

        self.build_read_envelope(
            ClientsSnapshotDto {
                generated_at_ms: now_ms,
                client_cards,
                sources,
                next_cursor: result.next_cursor,
                summary,
            },
            busytok_domain::now_ms(),
        )
    }

    async fn clients_detail(
        &self,
        req: ClientSourceDetailRequestDto,
    ) -> Result<ReadEnvelopeDto<ClientSourceDetailDto>> {
        let now_ms = busytok_domain::now_ms();
        let source_id = req.source_id;
        let source_id_for_read = source_id.clone();
        let (source_row, recent_activity_rows) = self
            .run_read_with_mode("clients.detail", "clients_detail", false, move |conn| {
                let source = busytok_store::read_queries::read_client_source_detail(
                    conn,
                    &source_id_for_read,
                )?
                .ok_or_else(|| anyhow::anyhow!("source not found: {}", source_id_for_read))?;
                let recent_activity =
                    busytok_store::read_queries::read_client_source_recent_activity(
                        conn,
                        &source_id_for_read,
                        10,
                    )?;
                Ok((source, recent_activity))
            })
            .await?;

        let source = ClientSourceRowDto {
            id: source_row.source_id.clone(),
            client_id: source_row.agent.clone(),
            client_label: ui_models::client_label(&source_row.agent),
            root_path: source_row.root_path.clone(),
            source_type: match source_row.source_type.as_str() {
                "manual" => SourceTypeDto::ManualRoot,
                _ => SourceTypeDto::DefaultDiscovery,
            },
            scan_state: ui_models::source_scan_state(&source_row.status),
            configured_by_user: source_row.configured_by_user,
            last_scan_at_ms: source_row.last_scan_at_ms,
            file_count: source_row.file_count,
            parsed_file_count: source_row.parsed_file_count,
            event_count: source_row.event_count,
            last_error: source_row.last_error.clone(),
        };

        let recent_activity = recent_activity_rows
            .iter()
            .map(Self::activity_item_from_read_row)
            .collect::<Vec<_>>();

        // Technical details
        let mut technical_details = Vec::new();
        if let Some(last_error) = &source_row.last_error {
            technical_details.push(TechnicalDetailDto {
                label: "Last Error".to_string(),
                value: last_error.clone(),
            });
        }
        technical_details.push(TechnicalDetailDto {
            label: "Files".to_string(),
            value: format!(
                "{} parsed / {} total",
                source_row.parsed_file_count, source_row.file_count
            ),
        });
        technical_details.push(TechnicalDetailDto {
            label: "Events".to_string(),
            value: source_row.event_count.to_string(),
        });
        technical_details.push(TechnicalDetailDto {
            label: "Scan State".to_string(),
            value: source_row.status.clone(),
        });

        self.build_read_envelope(
            ClientSourceDetailDto {
                source,
                recent_activity,
                technical_details,
            },
            now_ms,
        )
    }

    // ── Settings ─────────────────────────────────────────────────────

    async fn settings_snapshot(&self) -> Result<ReadEnvelopeDto<SettingsSnapshotDto>> {
        let diagnostics = self.settings_diagnostics().await?.data;

        let settings = self.settings.lock().unwrap();
        self.build_read_envelope(
            SettingsSnapshotDto {
                timezone: settings.timezone.clone(),
                week_starts_on: WeekdayIndexDto::from_u8(settings.week_starts_on)
                    .unwrap_or(WeekdayIndexDto::MONDAY),
                discovery: SettingsDiscoveryDto {
                    claude_code_default_paths: settings.discovery.claude_code_default_paths,
                    codex_default_paths: settings.discovery.codex_default_paths,
                    manual_roots: settings
                        .discovery
                        .manual_roots
                        .iter()
                        .map(|r| ManualRootDto {
                            id: if r.id.is_empty() {
                                format!("manual_{}", busytok_domain::hash_short(&r.root_path))
                            } else {
                                r.id.clone()
                            },
                            client_id: r.client_id.clone(),
                            root_path: r.root_path.clone(),
                            source_type: SourceTypeDto::ManualRoot,
                        })
                        .collect(),
                },
                privacy: SettingsPrivacyDto {
                    local_only: settings.privacy.local_only,
                    redact_sensitive_values: settings.privacy.redact_sensitive_values,
                },
                prompt_palette_default_action: match settings.prompt_palette_default_action {
                    busytok_config::PromptDefaultAction::OnlyCopy => PromptActionDto::OnlyCopy,
                    busytok_config::PromptDefaultAction::OnlyPaste => PromptActionDto::OnlyPaste,
                    busytok_config::PromptDefaultAction::CopyAndPaste => {
                        PromptActionDto::CopyAndPaste
                    }
                },
                subagent: {
                    let profiles: Vec<ProfileDto> = settings
                        .subagent
                        .profiles
                        .iter()
                        .map(|(name, cfg)| profile_to_dto(name, cfg))
                        .collect();
                    SettingsSubagentDto {
                        enabled: settings.subagent.enabled,
                        profiles,
                    }
                },
                diagnostics,
                recovery_actions: vec![
                    SettingsRecoveryActionDto {
                        id: SettingsRecoveryActionIdDto::RescanAll,
                        label: "Rescan All Sources".to_string(),
                        description: "Re-scan all configured log sources through the writer actor"
                            .to_string(),
                        dangerous: false,
                    },
                    SettingsRecoveryActionDto {
                        id: SettingsRecoveryActionIdDto::RebuildRollups,
                        label: "Rebuild Rollups".to_string(),
                        description: "Recalculate aggregate rollup tables through the writer actor"
                            .to_string(),
                        dangerous: false,
                    },
                    SettingsRecoveryActionDto {
                        id: SettingsRecoveryActionIdDto::ResetFailedCheckpoints,
                        label: "Reset Failed Checkpoints".to_string(),
                        description: "Reset log file checkpoints that are in an error/failed state"
                            .to_string(),
                        dangerous: true,
                    },
                ],
            },
            busytok_domain::now_ms(),
        )
    }

    async fn settings_update(
        &self,
        req: SettingsUpdateRequestDto,
    ) -> Result<ReadEnvelopeDto<SettingsSnapshotDto>> {
        // Collect validation errors for structured reporting.
        let mut errors: Vec<SettingsValidationErrorDto> = Vec::new();

        // Validate timezone if provided.
        if let Some(ref tz) = req.timezone {
            if tz.is_empty() {
                errors.push(SettingsValidationErrorDto {
                    code: SettingsValidationErrorCodeDto::InvalidTimezone,
                    field_path: "timezone".to_string(),
                    message: "Timezone must not be empty".to_string(),
                });
            } else if range::parse_timezone(tz).is_err() {
                errors.push(SettingsValidationErrorDto {
                    code: SettingsValidationErrorCodeDto::InvalidTimezone,
                    field_path: "timezone".to_string(),
                    message: format!("Invalid timezone '{}'", tz),
                });
            }
        }

        // Validate discovery if provided.
        if let Some(ref discovery) = req.discovery {
            for (i, root) in discovery.manual_roots.iter().enumerate() {
                if root.client_id.is_empty() {
                    errors.push(SettingsValidationErrorDto {
                        code: SettingsValidationErrorCodeDto::InvalidClientId,
                        field_path: format!("discovery.manual_roots[{}].client_id", i),
                        message: "Client ID must not be empty".to_string(),
                    });
                }
                if root.root_path.is_empty() {
                    errors.push(SettingsValidationErrorDto {
                        code: SettingsValidationErrorCodeDto::InvalidRootPath,
                        field_path: format!("discovery.manual_roots[{}].root_path", i),
                        message: "Root path must not be empty".to_string(),
                    });
                } else {
                    let p = Path::new(&root.root_path);
                    if !p.exists() {
                        errors.push(SettingsValidationErrorDto {
                            code: SettingsValidationErrorCodeDto::InvalidRootPath,
                            field_path: format!("discovery.manual_roots[{}].root_path", i),
                            message: format!("Root path '{}' does not exist", root.root_path),
                        });
                    }
                }
                // Check for duplicate (client_id, root_path) pairs.
                for (j, other) in discovery.manual_roots.iter().enumerate() {
                    if i != j
                        && root.client_id == other.client_id
                        && root.root_path == other.root_path
                        && !root.root_path.is_empty()
                    {
                        errors.push(SettingsValidationErrorDto {
                            code: SettingsValidationErrorCodeDto::DuplicateManualRoot,
                            field_path: "discovery.manual_roots".to_string(),
                            message: format!(
                                "Duplicate manual root '{}' for client '{}'",
                                root.root_path, root.client_id
                            ),
                        });
                    }
                }
            }
        }

        // If validation errors, bail with structured JSON payload.
        if !errors.is_empty() {
            let payload = serde_json::json!({"errors": errors});
            anyhow::bail!(
                "SETTINGS_VALIDATION_FAILED: {}",
                serde_json::to_string(&payload)?
            );
        }

        let (old_canonical, pending_settings, new_canonical) = {
            let settings = self.settings.lock().unwrap();
            let mut pending = settings.clone();

            let mut new_canonical: Option<String> = None;

            if let Some(ref tz) = req.timezone {
                // Canonicalize timezone (e.g. "local" -> IANA name, validate IANA names).
                // Validation already confirmed tz is parseable, so expect() is safe here.
                let rtz = ReportingTimezone::parse(tz)
                    .expect("timezone validated above but failed to parse");
                let canonical = rtz.canonical_name().to_string();
                new_canonical = Some(canonical.clone());
                pending.timezone = canonical;
            }

            let old_canonical = ReportingTimezone::parse(&settings.timezone)
                .map(|rtz| rtz.canonical_name().to_string())
                .unwrap_or_else(|_| settings.timezone.clone());

            if let Some(w) = req.week_starts_on {
                pending.week_starts_on = w.value();
            }
            if let Some(ref p) = req.privacy {
                pending.privacy.local_only = p.local_only;
                pending.privacy.redact_sensitive_values = p.redact_sensitive_values;
            }
            if let Some(ref discovery) = req.discovery {
                pending.discovery.claude_code_default_paths = discovery.claude_code_default_paths;
                pending.discovery.codex_default_paths = discovery.codex_default_paths;
                pending.discovery.manual_roots = discovery
                    .manual_roots
                    .iter()
                    .map(|r| {
                        let id = if r.id.is_empty() {
                            format!("manual_{}", busytok_domain::hash_short(&r.root_path))
                        } else {
                            r.id.clone()
                        };
                        busytok_config::ManualRootConfig {
                            id,
                            client_id: r.client_id.clone(),
                            root_path: r.root_path.clone(),
                        }
                    })
                    .collect();
            }

            if let Some(action) = req.prompt_palette_default_action {
                pending.prompt_palette_default_action = match action {
                    PromptActionDto::OnlyCopy => busytok_config::PromptDefaultAction::OnlyCopy,
                    PromptActionDto::OnlyPaste => busytok_config::PromptDefaultAction::OnlyPaste,
                    PromptActionDto::CopyAndPaste => {
                        busytok_config::PromptDefaultAction::CopyAndPaste
                    }
                };
            }

            (old_canonical, pending, new_canonical)
        };

        let timezone_changed = new_canonical
            .as_ref()
            .is_some_and(|new_tz| new_tz != &old_canonical);

        // Submit a SettingsWrite command to the writer for any key-level persistence
        // (e.g. timezone) that needs to be reflected in the write plane.
        // Only attempt when a Tokio runtime is active (skip in sync test contexts).
        if let Some(ref tz) = req.timezone {
            if tokio::runtime::Handle::try_current().is_ok() {
                let (respond_tx, respond_rx) = tokio::sync::oneshot::channel();
                let cmd = writer::WriteCommand::SettingsWrite(writer::SettingsWriteCommand {
                    key: "timezone".to_string(),
                    value_json: tz.clone(),
                    respond_tx,
                });
                self.writer_handle
                    .send(cmd)
                    .await
                    .map_err(|e| anyhow::anyhow!("failed to enqueue settings update: {e}"))?;
                // Wait for bounded commit with a 5-second timeout.
                match tokio::time::timeout(Duration::from_secs(5), respond_rx).await {
                    Ok(Ok(Ok(()))) => { /* writer committed successfully */ }
                    Ok(Ok(Err(e))) => {
                        return Err(anyhow::anyhow!("writer rejected settings update: {e}"));
                    }
                    Ok(Err(_)) => {
                        return Err(anyhow::anyhow!(
                            "writer dropped settings update response channel"
                        ));
                    }
                    Err(_) => {
                        return Err(anyhow::anyhow!(
                            "settings update writer commit timed out after 5s"
                        ));
                    }
                }
                if timezone_changed {
                    tokio::time::timeout(
                        Duration::from_secs(30),
                        self.writer_handle.rebuild_rollups(tz.clone()),
                    )
                    .await
                    .map_err(|_| {
                        anyhow::anyhow!("timezone rollup rebuild timed out after 30s")
                    })??;
                }
            } else if timezone_changed {
                return Err(anyhow::anyhow!(
                    "timezone changes require an active writer actor"
                ));
            }
        }

        pending_settings.save(&self.paths)?;

        {
            let mut settings = self.settings.lock().unwrap();
            *settings = pending_settings;
        }

        self.settings_snapshot().await
    }

    async fn settings_diagnostics(&self) -> Result<ReadEnvelopeDto<SettingsDiagnosticsDto>> {
        let db_data = self
            .run_read_or_fallback(
                "settings.diagnostics",
                "settings_diagnostics",
                false,
                |conn| {
                    let integrity: String = conn
                        .query_row("PRAGMA integrity_check", [], |r| r.get(0))
                        .unwrap_or_else(|_| "error".to_string());
                    let healthy = integrity == "ok";
                    let page_count: i64 = conn
                        .query_row("PRAGMA page_count", [], |r| r.get(0))
                        .unwrap_or(0);
                    let page_size: i64 = conn
                        .query_row("PRAGMA page_size", [], |r| r.get(0))
                        .unwrap_or(0);
                    let db_size_bytes = page_count * page_size;
                    let usage_event_count: i64 = conn
                        .query_row("SELECT COUNT(*) FROM usage_events", [], |r| r.get(0))
                        .unwrap_or(0);
                    let last_log_checkpoint_ms: Option<i64> = conn
                        .query_row("SELECT MAX(updated_at_ms) FROM log_files", [], |r| r.get(0))
                        .ok()
                        .flatten();
                    let migration_version: i32 = busytok_store::schema::SCHEMA_VERSION as i32;

                    let mut stmt = conn.prepare(
                        "SELECT id, severity, code, message, happened_at_ms \
                         FROM diagnostic_events ORDER BY happened_at_ms DESC LIMIT 100",
                    )?;
                    let diag_rows: Vec<(String, String, String, String, i64)> = stmt
                        .query_map([], |row| {
                            Ok((
                                row.get::<_, String>(0)?,
                                row.get::<_, String>(1)?,
                                row.get::<_, String>(2)?,
                                row.get::<_, String>(3)?,
                                row.get::<_, i64>(4)?,
                            ))
                        })?
                        .filter_map(|r| r.ok())
                        .collect();

                    Ok::<_, anyhow::Error>((
                        healthy,
                        db_size_bytes,
                        migration_version,
                        usage_event_count,
                        last_log_checkpoint_ms,
                        diag_rows,
                    ))
                },
            )
            .await?;

        let (
            db_healthy,
            db_size_bytes,
            migration_version,
            usage_event_count,
            last_log_checkpoint_ms,
            diag_rows,
        ) = db_data;

        let (writer_queue_depth, aggregate_lag_ms) = {
            let snap = self.status.try_read().ok();
            snap.map(|s| (s.writer_queue_depth, s.aggregate_lag_ms))
                .unwrap_or((0, 0))
        };

        let recent_diagnostics: Vec<SettingsDiagnosticEventDto> = diag_rows
            .iter()
            .filter(|(_, _, code, _, _)| {
                matches!(
                    code.as_str(),
                    "subscription_connected"
                        | "subscription_disconnected"
                        | "subscription_reconnect_failed"
                        | "writer_queue_depth_high"
                        | "aggregate_lag_exceeded"
                        | "rebuild_drift_detected"
                )
            })
            .map(
                |(_, severity, code, message, happened_at_ms)| SettingsDiagnosticEventDto {
                    code: code.clone(),
                    severity: severity.clone(),
                    message: message.clone(),
                    happened_at_ms: *happened_at_ms,
                },
            )
            .collect();

        // Spec §7.1 + §7.3: extend settings.diagnostics with subagent doctor
        // checks. Reuses the existing RPC path — no new method. Always
        // populate when the runtime is constructed; the per-check status
        // (e.g. sidecar_launchable "ok" when pi_sidecar.enabled=false)
        // reflects the current configuration rather than gating the whole
        // section. The DTO field is still `Option<...>` for wire-level
        // backwards-compat with older clients.
        let subagent = Some(self.run_subagent_doctor().await);

        let now_ms = busytok_domain::now_ms();
        self.build_read_envelope(
            SettingsDiagnosticsDto {
                db_healthy,
                db_size_bytes,
                migration_version: migration_version as i64,
                usage_event_count,
                last_log_checkpoint_ms,
                writer_queue_depth,
                aggregate_lag_ms,
                recent_diagnostics,
                subagent,
            },
            now_ms,
        )
    }

    async fn settings_recovery_action(
        &self,
        req: SettingsRecoveryActionRequestDto,
    ) -> Result<ReadEnvelopeDto<SettingsRecoveryActionResponseDto>> {
        let response = match req.id {
            SettingsRecoveryActionIdDto::RescanAll => {
                // TODO(#writer-backed-rescan-job): implement this as a dedicated
                // background job after the scan pipeline stops holding Database
                // references across await points. Calling run_initial_scan() from
                // this RuntimeControl method makes the future non-Send.
                SettingsRecoveryActionResponseDto {
                    id: req.id,
                    accepted: false,
                    message: "Full rescan requires the writer-backed background job path"
                        .to_string(),
                }
            }
            SettingsRecoveryActionIdDto::RebuildRollups => {
                let timezone = self.settings.lock().unwrap().timezone.clone();
                if tokio::runtime::Handle::try_current().is_err() {
                    SettingsRecoveryActionResponseDto {
                        id: req.id,
                        accepted: false,
                        message: "Rollup rebuild requires an active writer actor".to_string(),
                    }
                } else {
                    match tokio::time::timeout(
                        Duration::from_secs(30),
                        self.writer_handle.rebuild_rollups(timezone),
                    )
                    .await
                    {
                        Ok(Ok(())) => SettingsRecoveryActionResponseDto {
                            id: req.id,
                            accepted: true,
                            message: "Rollups rebuilt through writer actor".to_string(),
                        },
                        Ok(Err(err)) => SettingsRecoveryActionResponseDto {
                            id: req.id,
                            accepted: false,
                            message: format!("Rollup rebuild failed: {err}"),
                        },
                        Err(_) => SettingsRecoveryActionResponseDto {
                            id: req.id,
                            accepted: false,
                            message: "Rollup rebuild timed out after 30s".to_string(),
                        },
                    }
                }
            }
            SettingsRecoveryActionIdDto::ResetFailedCheckpoints => {
                if tokio::runtime::Handle::try_current().is_err() {
                    SettingsRecoveryActionResponseDto {
                        id: req.id,
                        accepted: false,
                        message: "Resetting failed checkpoints requires an active writer actor"
                            .to_string(),
                    }
                } else {
                    match tokio::time::timeout(
                        Duration::from_secs(5),
                        self.writer_handle.reset_failed_checkpoints(),
                    )
                    .await
                    {
                        Ok(Ok(updated)) => SettingsRecoveryActionResponseDto {
                            id: req.id,
                            accepted: true,
                            message: format!("Reset {updated} failed checkpoints"),
                        },
                        Ok(Err(err)) => SettingsRecoveryActionResponseDto {
                            id: req.id,
                            accepted: false,
                            message: format!("Failed to reset failed checkpoints: {err}"),
                        },
                        Err(_) => SettingsRecoveryActionResponseDto {
                            id: req.id,
                            accepted: false,
                            message: "Resetting failed checkpoints timed out after 5s".to_string(),
                        },
                    }
                }
            }
        };
        self.build_read_envelope(response, busytok_domain::now_ms())
    }

    // ── Live ──────────────────────────────────────────────────────────

    async fn live_window(
        &self,
        req: LiveWindowRequestDto,
    ) -> Result<ReadEnvelopeDto<LiveWindowDto>> {
        let now_ms = busytok_domain::now_ms();
        // Default to 15-minute chart horizon (450 buckets at 2s interval),
        // matching the 2-second live curve window from the spec.
        let window_seconds = req.window_seconds.unwrap_or(900);
        let (start_ms, end_ms) = Self::live_bucket_range(now_ms, window_seconds);
        let active_generation_id = {
            let snap = self.status.try_read().ok();
            snap.and_then(|s| s.active_generation_id.clone())
        };

        let query_gen_id = active_generation_id.clone();
        let query_start = start_ms;
        let query_end = end_ms;
        let sparse_exact = self
            .run_read_or_fallback("live.window", "live_window", true, move |conn| {
                let buckets = if let Some(gen_id) = query_gen_id.as_deref() {
                    busytok_store::live_queries::query_exact_buckets_range(
                        conn,
                        gen_id,
                        query_start,
                        query_end,
                    )
                    .unwrap_or_default()
                } else {
                    busytok_store::live_queries::query_backfill_buckets_range(
                        conn,
                        query_start,
                        query_end,
                    )
                    .unwrap_or_default()
                };
                Ok::<_, anyhow::Error>(buckets)
            })
            .await
            .unwrap_or_default();
        let exact_samples = Self::densify_live_samples(start_ms, end_ms, sparse_exact);

        let current_tokens_per_sec = exact_samples
            .last()
            .map(|s| s.tokens_per_sec)
            .unwrap_or(0.0);
        let current_events_per_sec = exact_samples
            .last()
            .map(|s| s.events_per_sec)
            .unwrap_or(0.0);

        // Read transient samples from the in-memory ring buffer.
        let transient_samples: Vec<LiveSampleDto> = {
            let snap = self.status.try_read().ok();
            snap.map(|s| s.transient_ring_buffer.iter().cloned().collect())
                .unwrap_or_default()
        };

        self.build_read_envelope(
            LiveWindowDto {
                exact_samples,
                transient_samples,
                current_tokens_per_sec,
                current_events_per_sec,
                start_ms,
                end_ms,
            },
            now_ms,
        )
    }

    // ── Prompts ───────────────────────────────────────────────────────

    async fn prompts_list(
        &self,
        req: PromptListQueryDto,
    ) -> Result<ReadEnvelopeDto<PromptListResponseDto>> {
        let query = req.query.clone();
        let tag = req.tag.clone();
        let sort = req.sort.unwrap_or(PromptSortDto::Smart);
        let limit = req.limit.unwrap_or(PROMPT_LIST_DEFAULT_LIMIT);
        tracing::debug!(
            operation = "prompts.list",
            has_query = query.is_some(),
            query_len = query.as_ref().map(|value| value.chars().count()).unwrap_or(0),
            has_tag = tag.is_some(),
            tag_len = tag.as_ref().map(|value| value.chars().count()).unwrap_or(0),
            sort = ?sort,
            limit,
            "listing prompt entries"
        );

        let db = self.prompt_database()?;
        let result = db.list_prompt_entries(prompt_list_query_to_row(req))?;
        let returned_count = result.entries.len();
        let total_count = result.total_count;
        let generated_at_ms = busytok_domain::now_ms();
        let response = PromptListResponseDto {
            entries: result
                .entries
                .into_iter()
                .map(prompt_entry_to_dto)
                .collect(),
            total_count,
        };

        tracing::debug!(
            operation = "prompts.list",
            has_query = query.is_some(),
            query_len = query.as_ref().map(|value| value.chars().count()).unwrap_or(0),
            has_tag = tag.is_some(),
            tag_len = tag.as_ref().map(|value| value.chars().count()).unwrap_or(0),
            sort = ?sort,
            limit,
            returned_count,
            total_count,
            "listed prompt entries"
        );

        self.build_read_envelope(response, generated_at_ms)
    }

    async fn prompts_get(
        &self,
        req: PromptGetRequestDto,
    ) -> Result<ReadEnvelopeDto<PromptEntryDto>> {
        let prompt_entry_id = req.id;
        tracing::debug!(
            operation = "prompts.get",
            prompt_entry_id = %prompt_entry_id,
            "loading prompt entry"
        );

        let db = self.prompt_database()?;
        let row = db
            .get_prompt_entry(&prompt_entry_id)?
            .ok_or_else(|| anyhow::anyhow!("prompt entry not found: {prompt_entry_id}"))?;
        let generated_at_ms = busytok_domain::now_ms();
        let response = prompt_entry_to_dto(row);

        tracing::debug!(
            operation = "prompts.get",
            prompt_entry_id = %prompt_entry_id,
            "loaded prompt entry"
        );

        self.build_read_envelope(response, generated_at_ms)
    }

    async fn prompts_create(
        &self,
        req: PromptCreateRequestDto,
    ) -> Result<ReadEnvelopeDto<PromptEntryDto>> {
        tracing::info!(operation = "prompts.create", "creating prompt entry");
        let db = self.prompt_database()?;
        let row = busytok_store::NewPromptEntryRow {
            content: req.content,
            alias: req.alias,
            tags: req.tags,
        };
        let entry = db.create_prompt_entry(row)?;
        let dto = prompt_entry_to_dto(entry);
        tracing::info!(
            operation = "prompts.create",
            prompt_entry_id = %dto.id,
            "created prompt entry"
        );
        let generated_at_ms = busytok_domain::now_ms();
        self.build_read_envelope(dto, generated_at_ms)
    }

    async fn prompts_update(
        &self,
        req: PromptUpdateRequestDto,
    ) -> Result<ReadEnvelopeDto<PromptEntryDto>> {
        let prompt_entry_id = req.id.clone();

        tracing::info!(
            operation = "prompts.update",
            prompt_entry_id = %prompt_entry_id,
            "updating prompt entry"
        );

        let db = self.prompt_database()?;
        let row = db.update_prompt_entry(busytok_store::UpdatePromptEntryRow {
            id: req.id,
            content: req.content,
            alias: req.alias,
            tags: req.tags,
            is_pinned: req.is_pinned,
        })?;
        let generated_at_ms = busytok_domain::now_ms();
        let response = prompt_entry_to_dto(row);

        tracing::info!(
            operation = "prompts.update",
            prompt_entry_id = %prompt_entry_id,
            "updated prompt entry"
        );

        self.build_read_envelope(response, generated_at_ms)
    }

    async fn prompts_delete(&self, req: PromptDeleteRequestDto) -> Result<PromptDeleteResultDto> {
        let prompt_entry_id = req.id;
        tracing::info!(
            operation = "prompts.delete",
            prompt_entry_id = %prompt_entry_id,
            "deleting prompt entry"
        );

        let db = self.prompt_database()?;
        let deleted = db.delete_prompt_entry(&prompt_entry_id)?;

        tracing::info!(
            operation = "prompts.delete",
            prompt_entry_id = %prompt_entry_id,
            deleted,
            "deleted prompt entry"
        );

        Ok(PromptDeleteResultDto { deleted })
    }

    async fn prompts_use(&self, req: PromptUseRequestDto) -> Result<PromptUseResultDto> {
        let prompt_entry_id = req.id;
        let action = req.action;
        let surface = req.surface;
        let outcome = req.outcome;
        let failure_reason = req.failure_reason;

        tracing::info!(
            operation = "prompts.use",
            prompt_entry_id = %prompt_entry_id,
            action = ?action,
            surface = ?surface,
            outcome = ?outcome,
            failure_reason = ?failure_reason,
            "recording prompt use"
        );

        let db = self.prompt_database()?;
        let result = db.record_prompt_use(busytok_store::PromptUseRow {
            prompt_entry_id: prompt_entry_id.clone(),
            action: prompt_action_to_row(action),
            surface: prompt_use_surface_to_row(surface),
            outcome: prompt_use_outcome_to_row(outcome),
            failure_reason: failure_reason.map(prompt_use_failure_reason_to_row),
        })?;

        tracing::info!(
            operation = "prompts.use",
            prompt_entry_id = %prompt_entry_id,
            action = ?action,
            surface = ?surface,
            outcome = ?outcome,
            failure_reason = ?failure_reason,
            usage_count = result.usage_count,
            "recorded prompt use"
        );

        Ok(PromptUseResultDto {
            usage_count: result.usage_count,
            last_used_at_ms: result.last_used_at_ms,
        })
    }

    async fn suggest_tags(
        &self,
        req: PromptSuggestTagsRequestDto,
    ) -> Result<PromptSuggestTagsResponseDto> {
        let query = req
            .query
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let limit = req.limit.unwrap_or(20);
        tracing::debug!(
            operation = "prompts.suggest_tags",
            query = ?query,
            limit,
            "suggesting tags"
        );

        let db = self.prompt_database()?;
        let prefix = query.unwrap_or("");
        let tags = db.suggest_tags(prefix, limit)?;

        tracing::debug!(
            operation = "prompts.suggest_tags",
            query = ?query,
            limit,
            match_count = tags.len(),
            "suggested tags"
        );

        Ok(PromptSuggestTagsResponseDto { tags })
    }

    // ── Subagents ────────────────────────────────────────────────────

    async fn subagent_delegate(
        &self,
        req: busytok_protocol::dto::SubagentDelegateRequestDto,
    ) -> Result<SubagentDelegateResponseDto> {
        // Spec §3.3 + Task 4: bound_provider_id + bound_model_id are read
        // from the DTO. For the reuse path (existing subagent by name or
        // subagent_id), the manager IGNORES the DTO bound fields and uses
        // the subagent's stored bound fields. For the create path, the
        // manager enforces "both or neither" against the DTO bound fields.
        //
        // Phase 3 Task 5: capture cwd before `req` is consumed by
        // `delegate_request_from_dto`, then bridge the subagent task's usage
        // into the unified `usage_events` pipeline after delegate returns.
        // The write is best-effort — failure is logged at `warn` but does NOT
        // fail the task (the task result is already persisted; usage is
        // best-effort observability).
        let cwd = req.cwd.clone();
        let r = self
            .subagent_manager()
            .delegate(delegate_request_from_dto(req))
            .await
            .map_err(map_subagent_error)?;
        // Only bridge usage for completed/failed tasks — queued tasks have
        // no usage yet (the dispatcher will write it when the task runs).
        if r.status != busytok_subagent::models::TaskStatus::Queued {
            if let Err(e) = self.write_subagent_usage_event(&r, &cwd) {
                tracing::warn!(
                    event_code = "subagent.usage_write_failed",
                    task_id = %r.task_id,
                    error = %e,
                    "failed to write subagent usage event to unified usage_events"
                );
            }
        }
        Ok(SubagentDelegateResponseDto {
            task_id: r.task_id,
            subagent_id: r.subagent_id,
            subagent_name: r.subagent_name,
            adapter: r.adapter,
            adapter_session_id: r.adapter_session_id,
            session_reused: r.session_reused,
            status: r.status.as_str().to_string(),
            profile: r.profile,
            model: r.model,
            summary: r.summary,
            usage: SubagentUsageDto {
                model: r.usage.model,
                provider: r.usage.provider,
                input_tokens: r.usage.input_tokens,
                output_tokens: r.usage.output_tokens,
                cache_read_tokens: r.usage.cache_read_tokens,
                cache_write_tokens: r.usage.cache_write_tokens,
                cost_usd: r.usage.cost_usd,
            },
        })
    }

    async fn subagent_list(&self, req: SubagentListRequestDto) -> Result<SubagentListResponseDto> {
        let status = req.status.as_deref().and_then(|s| s.parse().ok());
        let subs = self
            .subagent_manager()
            .list(
                status,
                req.project.as_deref(),
                req.include_deleted.unwrap_or(false),
            )
            .await
            .map_err(map_subagent_error)?;
        Ok(SubagentListResponseDto {
            subagents: subs.into_iter().map(subagent_detail).collect(),
        })
    }

    async fn subagent_show(
        &self,
        req: busytok_protocol::dto::SubagentResolveRequestDto,
    ) -> Result<SubagentDetailDto> {
        let s = self
            .subagent_manager()
            .show(resolve_params_from_dto(req))
            .await
            .map_err(map_subagent_error)?;
        Ok(subagent_detail(s))
    }

    async fn subagent_tasks(
        &self,
        req: SubagentTasksRequestDto,
    ) -> Result<SubagentTasksResponseDto> {
        let resolve = busytok_subagent::models::ResolveParams {
            name: req.name,
            id: req.id,
            cwd: req.cwd,
        };
        let tasks = self
            .subagent_manager()
            .tasks(resolve, req.limit.unwrap_or(20))
            .await
            .map_err(map_subagent_error)?;
        Ok(SubagentTasksResponseDto {
            tasks: tasks.into_iter().map(subagent_task_summary).collect(),
        })
    }

    async fn subagent_hibernate(
        &self,
        req: busytok_protocol::dto::SubagentResolveRequestDto,
    ) -> Result<SubagentAckDto> {
        let id = self
            .subagent_manager()
            .hibernate(resolve_params_from_dto(req))
            .await
            .map_err(map_subagent_error)?;
        Ok(SubagentAckDto {
            id,
            status: "hibernated".to_string(),
        })
    }

    async fn subagent_delete(&self, req: SubagentDeleteRequestDto) -> Result<SubagentAckDto> {
        let resolve = busytok_subagent::models::ResolveParams {
            name: req.name,
            id: req.id,
            cwd: req.cwd,
        };
        let id = self
            .subagent_manager()
            .delete(resolve, req.hard.unwrap_or(false))
            .await
            .map_err(map_subagent_error)?;
        Ok(SubagentAckDto {
            id,
            status: "deleted".to_string(),
        })
    }

    async fn subagent_runtime_status(
        &self,
        _req: SubagentRuntimeStatusRequestDto,
    ) -> Result<ReadEnvelopeDto<SubagentRuntimeStatusDto>> {
        let now_ms = now_ms();

        // 1. Aggregate worker snapshots across all providers in the pool
        //    (Phase 3 Task 4). When `worker_pool` is `Some` (sidecar enabled
        //    + config resolved), `pool.worker_snapshots()` returns one
        //    `(provider_id, WorkerSnapshot)` pair per spawned worker —
        //    covering the multi-provider case. When `worker_pool` is `None`
        //    but `sidecar_supervisor` is `Some` (legacy/degraded path that
        //    shouldn't normally occur since Task 3 wires both together), fall
        //    back to a single snapshot with `provider_id: None`. When neither
        //    is set (sidecar disabled), `worker_snaps` is empty → `workers: []`
        //    + default pressure_gate.
        //
        //    Lock-ordering: `pool.worker_snapshots()` collects `(pid, Arc<sup>)`
        //    pairs under the pool's map lock, DROPS the lock, then calls
        //    `sup.worker_snapshot().await` on each OUTSIDE the lock — safe.
        let worker_snaps: Vec<(Option<String>, busytok_subagent::sidecar::WorkerSnapshot)> =
            if let Some(pool) = self.worker_pool() {
                pool.worker_snapshots()
                    .await
                    .into_iter()
                    .map(|(pid, s)| (Some(pid), s))
                    .collect()
            } else if let Some(sup) = self.sidecar_supervisor() {
                match sup.worker_snapshot().await {
                    Some(s) => vec![(None, s)],
                    None => Vec::new(),
                }
            } else {
                Vec::new()
            };

        // 2. Read `hot_sessions_limit` from settings (default 3).
        let hot_sessions_limit = {
            let settings = self.settings.lock().unwrap();
            settings.subagent.pi_sidecar.max_hot_sessions
        };

        // 3. Build pressure_gate DTO by aggregating across all workers
        //    (Phase 3 Task 4, I4 fix). Aggregation rules:
        //    - `level`: max severity across all workers (Normal < Throttled
        //      < Evicting < Restarting via `PressureLevel::severity()`).
        //    - `memory_used_pct`: max across all workers.
        //    - `hot_sessions_total`: SUM across all workers.
        //    - `worker_sampled_at_ms`: most recent (max) `sampled_at_ms`
        //      across all workers — exposes the freshest sample.
        //    When `worker_snaps` is empty, defaults to `normal` / zeros.
        let pressure_gate = if worker_snaps.is_empty() {
            SubagentPressureGateDto {
                level: "normal".to_string(),
                memory_used_pct: 0,
                hot_sessions_total: 0,
                hot_sessions_limit,
                worker_sampled_at_ms: None,
            }
        } else {
            let mut max_severity: u8 = 0;
            let mut max_level: busytok_subagent::sidecar::PressureLevel =
                busytok_subagent::sidecar::PressureLevel::Normal;
            let mut max_memory_pct: u32 = 0;
            let mut hot_sessions_total: u32 = 0;
            let mut latest_sampled_at_ms: Option<i64> = None;
            for (_, snap) in &worker_snaps {
                let sev = snap.pressure_level.severity();
                if sev >= max_severity {
                    max_severity = sev;
                    max_level = snap.pressure_level;
                }
                if let Some(pct) = snap.memory_used_pct {
                    if pct > max_memory_pct {
                        max_memory_pct = pct;
                    }
                }
                hot_sessions_total = hot_sessions_total.saturating_add(snap.hot_sessions);
                match (latest_sampled_at_ms, snap.sampled_at_ms) {
                    (Some(cur), Some(new)) if new > cur => latest_sampled_at_ms = Some(new),
                    (None, Some(new)) => latest_sampled_at_ms = Some(new),
                    _ => {}
                }
            }
            let level = match max_level {
                busytok_subagent::sidecar::PressureLevel::Normal => "normal",
                busytok_subagent::sidecar::PressureLevel::Throttled => "throttled",
                busytok_subagent::sidecar::PressureLevel::Evicting => "evicting",
                busytok_subagent::sidecar::PressureLevel::Restarting => "restarting",
            };
            SubagentPressureGateDto {
                level: level.to_string(),
                memory_used_pct: max_memory_pct,
                hot_sessions_total,
                hot_sessions_limit,
                worker_sampled_at_ms: latest_sampled_at_ms,
            }
        };

        // 4. Build workers[] DTO — one row per worker snapshot, with the
        //    provider_id from the pool key (or `None` for the legacy
        //    single-supervisor fallback path).
        let workers: Vec<SubagentWorkerDto> = worker_snaps
            .iter()
            .map(|(provider_id, snap)| {
                let state = match snap.state {
                    busytok_subagent::sidecar::WorkerState::Running => "running",
                    busytok_subagent::sidecar::WorkerState::Stopped => "stopped",
                };
                SubagentWorkerDto {
                    provider_id: provider_id.clone(),
                    state: state.to_string(),
                    pid: snap.pid,
                    uptime_seconds: snap.uptime_seconds,
                    hot_sessions: snap.hot_sessions,
                }
            })
            .collect();

        // 5. Single-read aggregate DB read (one DB lock, all 4 queries —
        //    spec §4 line 213). The DB portion is internally consistent;
        //    the worker portion may be from a slightly earlier moment,
        //    exposed via `worker_sampled_at_ms`.
        let snapshot = self
            .subagent_manager()
            .runtime_status_snapshot(20)
            .await
            .map_err(map_subagent_error)?;

        // 6. Build subagents[] DTO. Join logical subagents with their
        //    task_count and last_task (created_at, status).
        let subagents: Vec<SubagentRuntimeSubagentDto> = snapshot
            .subagents
            .iter()
            .map(|s| {
                let task_count = snapshot.task_counts.get(&s.id).copied().unwrap_or(0);
                let last_task = snapshot.last_tasks.get(&s.id);
                SubagentRuntimeSubagentDto {
                    name: s.name.clone(),
                    status: s.status.as_str().to_string(),
                    task_count,
                    last_task_at_ms: last_task.map(|(ts, _)| *ts),
                    last_task_status: last_task.map(|(_, st)| st.clone()),
                }
            })
            .collect();

        // 7. Build tasks_recent[] DTO. Resolve `subagent_name` via the
        //    id→name lookup from `runtime_status_snapshot` — this includes
        //    ALL subagents (even deleted), so task history shows display
        //    names regardless of delete status (reviewer P1-2: decouple
        //    display name from delete filtering).
        let tasks_recent: Vec<SubagentRuntimeTaskDto> = snapshot
            .recent_tasks
            .iter()
            .map(|t| SubagentRuntimeTaskDto {
                task_id: t.id.clone(),
                subagent_name: snapshot
                    .name_lookup
                    .get(&t.subagent_id)
                    .cloned()
                    .unwrap_or_else(|| t.subagent_id.clone()),
                status: t.status.as_str().to_string(),
                created_at_ms: t.created_at_ms,
                error: t.error.clone(),
            })
            .collect();

        tracing::debug!(
            event_code = "subagent.runtime_status_served",
            subagent_count = subagents.len(),
            task_count = tasks_recent.len(),
            worker_count = workers.len(),
            "served subagent.runtime_status"
        );

        // 8. Wrap in ReadEnvelopeDto via `build_read_envelope` — reuses the
        //    existing envelope infrastructure (readiness / is_exact / is_stale
        //    / degraded_reason / generation_id / watermark_ms / progress from
        //    `ServiceStatusSnapshot`).
        self.build_read_envelope(
            SubagentRuntimeStatusDto {
                pressure_gate,
                subagents,
                tasks_recent,
                workers,
            },
            now_ms,
        )
    }

    // ── Providers (Phase 1: Credential Foundation) ──────────────────

    async fn provider_create(&self, req: ProviderCreateRequestDto) -> Result<ProviderDto> {
        // id 由 store 层生成（UUID v4），不再校验用户输入。api_key 直接
        // 存入 SQL providers 表。
        let provider = {
            let db = self.db.lock().unwrap();
            db.create_provider(busytok_store::CreateProviderReq {
                name: req.name,
                provider_kind: req.provider_kind,
                base_url: req.base_url,
                enabled: req.enabled.unwrap_or(true),
                api_key: req.api_key,
            })
            .map_err(|e| {
                tracing::error!(event_code = "provider.sql_write_failed", error = %e, "create_provider failed");
                e
            })?
        };
        tracing::info!(event_code = "provider.created", provider_id = %provider.id, "provider created");
        // Defensive: kill any pre-existing worker for this provider id.
        // Typically a no-op (brand-new provider has no worker yet), but
        // covers the edge case where a provider was deleted without the
        // worker being cleaned up, then re-created — the stale worker would
        // hold the OLD credentials.
        self.provider_changed(&provider.id).await;
        Ok(provider_to_dto(&provider))
    }

    async fn provider_list(&self) -> Result<ProviderListResponseDto> {
        // SQL-backed: list_providers returns ProviderSummary (no api_key).
        // The settings lock is NOT taken — provider CRUD no longer touches
        // settings.toml.
        let summaries = {
            let db = self.db.lock().unwrap();
            db.list_providers().map_err(|e| {
                tracing::error!(event_code = "provider.sql_read_failed", error = %e, "list_providers failed");
                e
            })?
        };
        let providers: Vec<ProviderDto> = summaries.iter().map(provider_summary_to_dto).collect();
        tracing::debug!(count = providers.len(), "listed providers");
        Ok(ProviderListResponseDto { providers })
    }

    async fn provider_update(&self, req: ProviderUpdateRequestDto) -> Result<ProviderDto> {
        // SQL-backed: api_key is three-state Option<Option<String>>.
        //   None              → no change
        //   Some(None)        → clear
        //   Some(Some(k))     → update
        // The store layer handles all three branches atomically under a
        // transaction.
        let provider = {
            let db = self.db.lock().unwrap();
            db.update_provider(&req.id, busytok_store::UpdateProviderPatch {
                name: req.name,
                base_url: req.base_url,
                enabled: req.enabled,
                // Spec §7.1: patching provider_kind changes the API shape
                // (openai_completions vs anthropic_messages). The worker is
                // killed below via `provider_changed` so the next delegate
                // re-spawns it with the new API shape.
                provider_kind: req.provider_kind,
                api_key: req.api_key,
            })
            .map_err(|e| {
                tracing::error!(event_code = "provider.sql_write_failed", provider_id = %req.id, error = %e, "update_provider failed");
                e
            })?
        };
        tracing::info!(event_code = "provider.updated", provider_id = %provider.id, "provider updated");
        // Kill the worker so the next delegate re-spawns it with the updated
        // config (metadata changes like base_url AND key rotations). Safe
        // no-op when no worker exists for this provider.
        self.provider_changed(&provider.id).await;
        Ok(provider_to_dto(&provider))
    }

    async fn provider_delete(&self, req: ProviderDeleteRequestDto) -> Result<()> {
        // SQL-backed delete with "allow delete, dangling binding" semantics.
        // Per Task 3 spec §7.5: deleting a provider is always allowed even if
        // subagents still reference it via `bound_provider_id`. The resulting
        // dangling binding is reported at delegate time (fail-fast) — no
        // implicit unbind, no auto-rebind.
        {
            let db = self.db.lock().unwrap();
            db.delete_provider(&req.id).map_err(|e| {
                tracing::error!(event_code = "provider.sql_write_failed", provider_id = %req.id, error = %e, "delete_provider failed");
                e
            })?;
        }
        tracing::info!(event_code = "provider.deleted", provider_id = %req.id, "provider deleted");
        // Kill + remove the worker for the deleted provider so its sidecar
        // process doesn't keep running with stale credentials. Safe no-op
        // when no worker exists for this provider.
        self.provider_deleted(&req.id).await;
        Ok(())
    }

    async fn provider_test_connection(
        &self,
        req: ProviderTestConnectionRequestDto,
    ) -> Result<ProviderTestConnectionResponseDto> {
        // SQL-backed: load provider (with secret) from the store. Drop the db
        // guard before awaiting — holding a `MutexGuard` across `.await` makes
        // the future `!Send` and breaks the `RuntimeControl` trait bound.
        let provider = {
            let db = self.db.lock().unwrap();
            db.get_provider_with_secret(&req.id)
                .map_err(|e| {
                    tracing::error!(event_code = "provider.sql_read_failed", provider_id = %req.id, error = %e, "get_provider_with_secret failed");
                    e
                })?
                .ok_or_else(|| anyhow::anyhow!("provider not found: {}", req.id))?
        };
        let provider_id = provider.id.clone();
        let base_url = provider.base_url.clone();
        // Defense-in-depth: the frontend doesn't enforce HTTPS, so the backend
        // must reject cleartext URLs before reading the key or sending the key
        // in an Authorization header.
        if !base_url.starts_with("https://") {
            anyhow::bail!("provider base_url must use HTTPS (got: {})", base_url);
        }
        let api_key = provider
            .api_key
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("provider has no api key"))?;
        let url = format!("{}/models", base_url.trim_end_matches('/'));
        tracing::info!(
            event_code = "provider.test_connection",
            provider_id = %provider_id,
            url = %url,
            "testing provider connection"
        );
        // Disable redirects so the Authorization header is never forwarded to
        // a cross-origin host (a compromised endpoint could otherwise redirect
        // to an attacker-controlled host and exfiltrate the key).
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .redirect(reqwest::redirect::Policy::none())
            .build()?;
        let resp = client
            .get(&url)
            .header("Authorization", format!("Bearer {}", api_key))
            .send()
            .await;
        match resp {
            Ok(r) if r.status().is_success() => {
                tracing::info!(event_code = "provider.test_connection.ok", provider_id = %provider_id, "connection test succeeded");
                Ok(ProviderTestConnectionResponseDto {
                    ok: true,
                    error: None,
                    models_detected: None,
                })
            }
            Ok(r) => {
                let status = r.status();
                // Spec §4: if GET /v1/models is absent/unsupported (404/405/501),
                // fall back to POST /v1/chat/completions with a 1-token prompt.
                if models_probe_should_fallback(status) {
                    tracing::debug!(
                        event_code = "provider.test_connection.fallback",
                        provider_id = %provider_id,
                        models_status = %status,
                        "falling back to /chat/completions"
                    );
                    // Probe model comes from the SQL models table (enabled only).
                    let probe_model = {
                        let db = self.db.lock().unwrap();
                        db.list_models_filtered(busytok_domain::ModelCatalogFilter {
                            provider_id: Some(provider_id.clone()),
                            tags: vec![],
                            include_disabled: false,
                        })
                        .map_err(|e| {
                            tracing::error!(event_code = "model.sql_read_failed", provider_id = %provider_id, error = %e, "list_models_filtered for probe failed");
                            e
                        })?
                    };
                    let probe_model = probe_model.into_iter().next()
                        .ok_or_else(|| anyhow::anyhow!(
                            "provider has no enabled models configured, cannot probe /chat/completions"
                        ))?;
                    let chat_url = format!("{}/chat/completions", base_url.trim_end_matches('/'));
                    let body = serde_json::json!({
                        "model": probe_model.model_id,
                        "max_tokens": 1,
                        "messages": [{"role": "user", "content": "ping"}],
                    });
                    let chat_resp = client
                        .post(&chat_url)
                        .header("Authorization", format!("Bearer {}", api_key))
                        .json(&body)
                        .send()
                        .await;
                    return match chat_resp {
                        Ok(cr) => {
                            let cstatus = cr.status();
                            let (ok, error) = interpret_chat_probe(cstatus);
                            if ok {
                                tracing::info!(
                                    event_code = "provider.test_connection.ok",
                                    provider_id = %provider_id,
                                    "connection test succeeded via /chat/completions fallback"
                                );
                            } else {
                                tracing::warn!(
                                    event_code = "provider.test_connection.failed",
                                    provider_id = %provider_id,
                                    status = %cstatus,
                                    "connection test failed via /chat/completions fallback"
                                );
                            }
                            Ok(ProviderTestConnectionResponseDto {
                                ok,
                                error,
                                models_detected: None,
                            })
                        }
                        Err(e) => {
                            tracing::warn!(
                                event_code = "provider.test_connection.error",
                                provider_id = %provider_id,
                                error = %e,
                                "connection test error during /chat/completions fallback"
                            );
                            Ok(ProviderTestConnectionResponseDto {
                                ok: false,
                                error: Some(format!("request failed: {}", e)),
                                models_detected: None,
                            })
                        }
                    };
                }
                tracing::warn!(event_code = "provider.test_connection.failed", provider_id = %provider_id, status = %status, "connection test failed");
                Ok(ProviderTestConnectionResponseDto {
                    ok: false,
                    error: Some(format!("HTTP {}", status)),
                    models_detected: None,
                })
            }
            Err(e) => {
                tracing::warn!(event_code = "provider.test_connection.error", provider_id = %provider_id, error = %e, "connection test error");
                Ok(ProviderTestConnectionResponseDto {
                    ok: false,
                    error: Some(format!("request failed: {}", e)),
                    models_detected: None,
                })
            }
        }
    }

    // ── Models (Phase: Provider/Model Catalog Refactor) ─────────────
    //
    // SQL-backed model CRUD. The store layer (`busytok_store::provider_catalog`)
    // owns all SQL; the supervisor handlers below are thin adapters that
    // translate DTO ↔ store requests, drop the db lock before any `.await`
    // (the `RuntimeControl` trait requires `Send` futures), and emit
    // structured tracing events.
    //
    // `model_id` is immutable after creation — `ModelUpdateRequestDto` only
    // carries `id` + `enabled`. `model_delete` is blocking: profile refs are
    // collected from settings and the store rejects the delete if any profile
    // references this (provider_id, model_id) pair.

    async fn model_create(&self, req: ModelCreateRequestDto) -> Result<ModelCatalogEntryDto> {
        // Create the model row + initial tags atomically (store layer
        // transaction). `model_id` immutability is enforced at the store
        // layer via UNIQUE(provider_id, model_id). Spec §6.1: context_window
        // + max_tokens are required at create time (enforced by the DTO);
        // display_name + reasoning are optional.
        let model = {
            let db = self.db.lock().unwrap();
            db.create_model(busytok_store::CreateModelReq {
                provider_id: req.provider_id.clone(),
                model_id: req.model_id.clone(),
                enabled: req.enabled.unwrap_or(true),
                tags: req.tags.clone(),
                display_name: req.display_name.clone(),
                reasoning: req.reasoning,
                context_window: Some(req.context_window),
                max_tokens: Some(req.max_tokens),
            })
            .map_err(|e| {
                tracing::error!(event_code = "model.sql_write_failed", provider_id = %req.provider_id, model_id = %req.model_id, error = %e, "create_model failed");
                e
            })?
        };
        tracing::info!(
            event_code = "model.created",
            model_id = %model.model_id,
            provider_id = %model.provider_id,
            model_db_id = %model.id,
            "model created"
        );
        // Re-fetch via the catalog join so the DTO carries provider_name /
        // provider_kind / provider_enabled / tags in one shot. The store
        // returns the full joined row.
        let entries = {
            let db = self.db.lock().unwrap();
            db.list_models_filtered(busytok_domain::ModelCatalogFilter {
                provider_id: Some(model.provider_id.clone()),
                tags: vec![],
                include_disabled: true,
            })
            .map_err(|e| {
                tracing::error!(event_code = "model.sql_read_failed", provider_id = %model.provider_id, model_db_id = %model.id, error = %e, "list_models_filtered re-fetch after create failed");
                e
            })?
        };
        entries
            .into_iter()
            .find(|e| e.model_db_id == model.id)
            .map(catalog_entry_to_dto)
            .ok_or_else(|| anyhow::anyhow!("model not found after create"))
    }

    async fn model_list(&self, req: ModelListRequestDto) -> Result<ModelListResponseDto> {
        // No settings lock — model catalog lives entirely in SQL. Tag filter
        // uses AND semantics (a model must have ALL selected tags).
        let entries = {
            let db = self.db.lock().unwrap();
            db.list_models_filtered(busytok_domain::ModelCatalogFilter {
                provider_id: req.provider_id,
                tags: req.tags,
                include_disabled: req.include_disabled,
            })
            .map_err(|e| {
                tracing::error!(event_code = "model.sql_read_failed", error = %e, "list_models_filtered failed");
                e
            })?
        };
        tracing::info!(
            event_code = "model.catalog.listed",
            count = entries.len(),
            "model catalog listed"
        );
        Ok(ModelListResponseDto {
            models: entries.iter().map(catalog_entry_to_dto_ref).collect(),
        })
    }

    async fn model_update(&self, req: ModelUpdateRequestDto) -> Result<()> {
        // `model_id` is immutable — only enabled + metadata are patchable
        // (spec §6.1). The store layer's `update_model` enforces "model not
        // found" via row-count check (rows == 0 → bail).
        let updated = {
            let db = self.db.lock().unwrap();
            db.update_model(&req.id, busytok_store::UpdateModelPatch {
                enabled: req.enabled,
                display_name: req.display_name,
                reasoning: req.reasoning,
                context_window: req.context_window,
                max_tokens: req.max_tokens,
            })
            .map_err(|e| {
                tracing::error!(event_code = "model.sql_write_failed", model_db_id = %req.id, error = %e, "update_model failed");
                e
            })?
        };
        tracing::info!(
            event_code = "model.updated",
            model_db_id = %updated.id,
            model_id = %updated.model_id,
            enabled = updated.enabled,
            "model updated"
        );
        Ok(())
    }

    async fn model_delete(&self, req: ModelDeleteRequestDto) -> Result<()> {
        // "Allow delete, dangling binding" semantics: deleting a model is
        // always allowed even if subagents still reference it via
        // `bound_model_id`. The dangling binding is reported at delegate
        // time (fail-fast) — no auto-unbind, no implicit rebinding.
        {
            let db = self.db.lock().unwrap();
            db.delete_model(&req.id).map_err(|e| {
                tracing::error!(event_code = "model.sql_write_failed", model_db_id = %req.id, error = %e, "delete_model failed");
                e
            })?;
        }
        tracing::info!(
            event_code = "model.deleted",
            model_db_id = %req.id,
            "model deleted"
        );
        Ok(())
    }

    async fn model_tags_update(&self, req: ModelTagUpdateDto) -> Result<()> {
        // Replace-all semantics: the store diffs against existing tags and
        // only writes the delta (insert new, remove stale) inside a
        // transaction. `model_id` here is the models.id (DB PK), not the
        // immutable model_id string — same param name, different meaning.
        let db = self.db.lock().unwrap();
        db.set_model_tags(&req.model_id, &req.tags).map_err(|e| {
            tracing::error!(event_code = "model.sql_write_failed", model_db_id = %req.model_id, error = %e, "set_model_tags failed");
            e
        })?;
        tracing::info!(
            event_code = "model.tags_updated",
            model_db_id = %req.model_id,
            tag_count = req.tags.len(),
            "model tags updated"
        );
        Ok(())
    }

    // ── Phase 5: pi_sidecar locator (service-owned in-memory + on-disk update) ──
    //
    // Mirrors `provider_update`: clone → validate → mutate → save → swap →
    // rebuild. The daemon holds settings in `Arc<Mutex<BusytokSettings>>` — a
    // direct file write would leave the running daemon's in-memory state stale
    // ("file fixed, current session still can't find sidecar"). This method
    // atomically (1) validates the path, (2) persists to `settings.toml`,
    // (3) swaps the in-memory Arc, AND (4) rebuilds the sidecar runtime
    // in-flight so `doctor`/`delegate` work in the CURRENT session (spec
    // §472-475: fresh-install closed loop — no restart required).
    // The GUI calls this via the `pi_sidecar_locator_update` RPC on startup
    // (spec §371: refresh mechanism — the service owns the mutation).

    async fn pi_sidecar_locator_update(
        &self,
        req: PiSidecarLocatorUpdateRequestDto,
    ) -> Result<PiSidecarLocatorUpdateResponseDto> {
        // P1: validate runtime_dir BEFORE persisting. Don't write bad paths
        // to settings — that would leave the service in a broken state that
        // survives restart. Only validate when enabling (when disabling, the
        // path may be stale and we don't care about its contents).
        //
        // P2: validation now parses manifest.json as SidecarManifest +
        // verifies the manifest-referenced bundle + (when node_runtime=
        // "bundled") requires node/<current-arch>/node. This catches
        // malformed/broken packaged runtimes at the write path, before they
        // can be persisted and marked enabled — without this, the GUI could
        // accept a dir that only fails later during doctor/delegate.
        if req.enabled {
            let node_runtime = {
                let settings = self.settings.lock().unwrap();
                settings.subagent.pi_sidecar.node_runtime.clone()
            };
            validate_runtime_dir(&req.runtime_dir, &node_runtime)?;
        }

        let mut pending_settings = {
            let settings = self.settings.lock().unwrap();
            settings.clone()
        };

        let old_enabled = pending_settings.subagent.pi_sidecar.enabled;
        let changed = pending_settings.subagent.pi_sidecar.runtime_dir.as_deref()
            != Some(req.runtime_dir.as_str())
            || old_enabled != req.enabled;

        pending_settings.subagent.pi_sidecar.runtime_dir = Some(req.runtime_dir.clone());
        pending_settings.subagent.pi_sidecar.enabled = req.enabled;

        // Persist to disk BEFORE swapping in-memory (mirrors provider_update:
        // save first so a swap failure doesn't leave memory ahead of disk).
        pending_settings.save(&self.paths)?;

        // Swap the in-memory settings so `construct_sidecar` (called during
        // rebuild below) sees the new locator.
        {
            let mut settings = self.settings.lock().unwrap();
            *settings = pending_settings;
        }

        // P0: rebuild the sidecar runtime whenever `enabled` or `runtime_dir`
        // changed. This closes the spec §472-475 fresh-install loop: GUI calls
        // this RPC → service rebuilds sidecar in-flight → doctor/delegate work
        // in the CURRENT session. When DISABLING (enabled=true → false), the
        // rebuild swaps in a MockTaskExecutor runtime and shuts down the old
        // supervisor — without this, the old Node subprocess stays alive and
        // the doctor would misleadingly report "ok" (supervisor still running).
        let rebuilt = if changed {
            self.rebuild_sidecar_runtime().await;
            true
        } else {
            false
        };

        tracing::info!(
            event_code = "pi_sidecar.locator_updated",
            runtime_dir = %req.runtime_dir,
            enabled = req.enabled,
            changed = changed,
            rebuilt = rebuilt,
            "pi_sidecar locator updated (in-memory + on-disk)"
        );

        Ok(PiSidecarLocatorUpdateResponseDto {
            runtime_dir: req.runtime_dir,
            enabled: req.enabled,
            in_memory_updated: true,
        })
    }

    // ── Profiles (Phase 4: Profile/Model Configuration UI) ──────────

    async fn profile_create(&self, req: ProfileCreateRequestDto) -> Result<ProfileDto> {
        // Reject built-in profile names — they're reserved.
        if busytok_config::is_builtin_profile(&req.id) {
            tracing::warn!(event_code = "profile.create.rejected", profile_id = %req.id, reason = "reserved_name", "name is reserved for built-in profiles");
            anyhow::bail!(
                "cannot create profile '{}': name is reserved for built-in profiles",
                req.id
            );
        }
        if req.id.is_empty() {
            tracing::warn!(
                event_code = "profile.create.rejected",
                reason = "empty_id",
                "profile id must not be empty"
            );
            anyhow::bail!("profile id must not be empty");
        }
        // Validate id format: [a-z0-9/_-]+ (allows namespacing like "pi/my-profile").
        if !req.id.chars().all(|c| {
            c.is_ascii_lowercase() || c.is_ascii_digit() || c == '/' || c == '_' || c == '-'
        }) {
            tracing::warn!(event_code = "profile.create.rejected", profile_id = %req.id, reason = "invalid_id_format", "profile id must contain only [a-z0-9/_-]+");
            anyhow::bail!("profile id must contain only [a-z0-9/_-]+");
        }

        let mut pending = {
            let settings = self.settings.lock().unwrap();
            settings.clone()
        };
        if pending.subagent.profiles.contains_key(&req.id) {
            tracing::warn!(event_code = "profile.create.rejected", profile_id = %req.id, reason = "already_exists", "profile already exists");
            anyhow::bail!("profile already exists: {}", req.id);
        }

        // Task 3: Profile downgrade — provider_id / model fields are gone from
        // `SubagentProfileConfig`. Provider/model binding is now per-subagent
        // via SQL catalog (`subagent.bound_provider_id` / `bound_model_id`),
        // so the create path no longer validates a provider whitelist. The
        // new validation chain (Task 6) will validate bound fields at the
        // delegate path.
        let profile = busytok_config::SubagentProfileConfig {
            write_access: req.write_access.unwrap_or(false),
            tools: req.tools.unwrap_or_default(),
            context_budget_tokens: req.context_budget_tokens.unwrap_or(3000),
            timeout_seconds: req.timeout_seconds.unwrap_or(120),
        };

        pending
            .subagent
            .profiles
            .insert(req.id.clone(), profile.clone());
        pending.save(&self.paths)?;
        {
            let mut settings = self.settings.lock().unwrap();
            *settings = pending;
        }

        let dto = profile_to_dto(&req.id, &profile);
        tracing::info!(event_code = "profile.created", profile_id = %req.id, "profile created");
        Ok(dto)
    }

    async fn profile_update(&self, req: ProfileUpdateRequestDto) -> Result<ProfileDto> {
        let mut pending = {
            let settings = self.settings.lock().unwrap();
            settings.clone()
        };
        let profile = pending.subagent.profiles.get_mut(&req.id)
            .ok_or_else(|| {
                tracing::warn!(event_code = "profile.update.rejected", profile_id = %req.id, reason = "not_found", "profile not found");
                anyhow::anyhow!("profile not found: {}", req.id)
            })?;

        // Task 3: Profile downgrade — only behavior-template fields are
        // patchable. provider_id / model patches are no longer supported
        // (those fields no longer exist on `SubagentProfileConfig`). Provider
        // and model binding is per-subagent and mutated via the subagent
        // binding RPCs (Task 6), not via profile updates.
        if let Some(tools) = req.tools {
            profile.tools = tools;
        }
        if let Some(budget) = req.context_budget_tokens {
            profile.context_budget_tokens = budget;
        }
        if let Some(timeout) = req.timeout_seconds {
            profile.timeout_seconds = timeout;
        }
        if let Some(write_access) = req.write_access {
            profile.write_access = write_access;
        }

        let profile_snapshot = profile.clone();
        pending.save(&self.paths)?;
        {
            let mut settings = self.settings.lock().unwrap();
            *settings = pending;
        }

        let dto = profile_to_dto(&req.id, &profile_snapshot);
        tracing::info!(event_code = "profile.updated", profile_id = %req.id, "profile updated");
        Ok(dto)
    }

    async fn profile_delete(&self, req: ProfileDeleteRequestDto) -> Result<()> {
        // Reject deletion of built-in profiles.
        if busytok_config::is_builtin_profile(&req.id) {
            tracing::warn!(event_code = "profile.delete.rejected", profile_id = %req.id, reason = "builtin", "cannot delete built-in profile");
            anyhow::bail!("cannot delete built-in profile: {}", req.id);
        }

        let mut pending = {
            let settings = self.settings.lock().unwrap();
            settings.clone()
        };
        if !pending.subagent.profiles.contains_key(&req.id) {
            tracing::warn!(event_code = "profile.delete.rejected", profile_id = %req.id, reason = "not_found", "profile not found");
            anyhow::bail!("profile not found: {}", req.id);
        }

        pending.subagent.profiles.remove(&req.id);
        pending.save(&self.paths)?;
        {
            let mut settings = self.settings.lock().unwrap();
            *settings = pending;
        }

        tracing::info!(event_code = "profile.deleted", profile_id = %req.id, "profile deleted");
        Ok(())
    }

    // ── Events ───────────────────────────────────────────────────────

    fn event_bus(&self) -> &AppEventBus {
        &self.event_bus
    }

    fn latest_event_seq(&self) -> Option<i64> {
        // Read from the status snapshot without blocking.
        self.status.try_read().ok().and_then(|s| s.latest_event_seq)
    }

    fn record_diagnostic(&self, severity: &str, code: &str, message: &str) {
        let cmd = crate::writer::DiagnosticWriteCommand {
            source_id: "runtime".to_string(),
            severity: severity.to_string(),
            code: code.to_string(),
            message: message.to_string(),
            details_json: None,
        };
        // Use try_send so subscription lifecycle recording never stalls the server.
        let _ = self
            .writer_handle
            .try_send(crate::writer::WriteCommand::DiagnosticWrite(cmd));
    }
}

/// Pure decision helpers for `provider_test_connection`. Extracted so the
/// fallback logic (which status codes trigger a fallback, how the POST probe
/// status is interpreted) is unit-testable without standing up a TLS mock
/// server — the handler enforces HTTPS, which rules out plain-HTTP fakes.
///
/// Spec §4: probe `GET /v1/models` OR `POST /v1/chat/completions` with a
/// 1-token prompt.

/// Returns true when a `GET /models` failure should fall back to
/// `POST /chat/completions`. Only "endpoint absent/unsupported" codes
/// (404/405/501) trigger the fallback; auth or server errors are reported
/// directly because the endpoint itself is reachable.
fn models_probe_should_fallback(status: reqwest::StatusCode) -> bool {
    matches!(status.as_u16(), 404 | 405 | 501)
}

/// First model id to use for the `/chat/completions` probe body. Defaults to a
/// generic OpenAI model id when the provider's model whitelist is empty — the
/// probe only checks whether the endpoint accepts the request, so a 401/403
/// still means "connection works, auth issue".
fn chat_probe_model(models: &[String]) -> String {
    models
        .first()
        .cloned()
        .unwrap_or_else(|| "gpt-3.5-turbo".to_string())
}

/// Interprets the `POST /chat/completions` probe status. Returns `(ok, error)`.
fn interpret_chat_probe(status: reqwest::StatusCode) -> (bool, Option<String>) {
    if status.is_success() {
        (true, None)
    } else {
        let msg = match status.as_u16() {
            401 | 403 => "connection works but authentication failed".to_string(),
            404 | 405 | 501 => "provider does not support /models or /chat/completions".to_string(),
            _ => format!("HTTP {}", status),
        };
        (false, Some(msg))
    }
}

/// Maps a `busytok_domain::Provider` (SQL domain type, with secret) to a
/// `ProviderDto` (wire type). Never exposes `api_key` — only `has_api_key`.
fn provider_to_dto(p: &busytok_domain::Provider) -> ProviderDto {
    ProviderDto {
        id: p.id.clone(),
        name: p.name.clone(),
        provider_kind: p.provider_kind.clone(),
        base_url: p.base_url.clone(),
        enabled: p.enabled,
        has_api_key: p.api_key.is_some(),
        created_at_ms: p.created_at_ms,
        updated_at_ms: p.updated_at_ms,
    }
}

/// Maps a `busytok_domain::ProviderSummary` (SQL domain type, no secret) to a
/// `ProviderDto` (wire type). Used by `provider_list` which reads summaries.
fn provider_summary_to_dto(s: &busytok_domain::ProviderSummary) -> ProviderDto {
    ProviderDto {
        id: s.id.clone(),
        name: s.name.clone(),
        provider_kind: s.provider_kind.clone(),
        base_url: s.base_url.clone(),
        enabled: s.enabled,
        has_api_key: s.has_api_key,
        created_at_ms: s.created_at_ms,
        updated_at_ms: s.updated_at_ms,
    }
}

/// Maps a `busytok_domain::ModelCatalogEntry` (SQL joined row) to a
/// `ModelCatalogEntryDto` (wire type). Consumes the entry — used by
/// `model_create` which already owns the row from a fresh SQL fetch.
///
/// No `api_key` field anywhere in this mapping (only provider metadata +
/// model + tags) — safe to expose to the GUI/CLI.
fn catalog_entry_to_dto(e: busytok_domain::ModelCatalogEntry) -> ModelCatalogEntryDto {
    ModelCatalogEntryDto {
        provider_id: e.provider_id,
        provider_name: e.provider_name,
        provider_kind: e.provider_kind,
        provider_enabled: e.provider_enabled,
        model_db_id: e.model_db_id,
        model_id: e.model_id,
        model_enabled: e.model_enabled,
        tags: e.tags,
        // Spec §6.1: model metadata (display_name + reasoning + context_window
        // + max_tokens) surfaced on the catalog entry so the GUI/CLI can show
        // model capabilities without a separate fetch.
        display_name: e.display_name,
        reasoning: e.reasoning,
        context_window: e.context_window,
        max_tokens: e.max_tokens,
    }
}

/// By-reference variant used by `model_list`, which iterates over the
/// borrowed `Vec<ModelCatalogEntry>` from the store. Clones once (the DTO
/// owns its strings) — acceptable for catalog-sized lists.
fn catalog_entry_to_dto_ref(e: &busytok_domain::ModelCatalogEntry) -> ModelCatalogEntryDto {
    catalog_entry_to_dto(e.clone())
}

/// Maps a `SubagentProfileConfig` (settings-layer type) to a `ProfileDto`
/// (wire type). Mirrors `provider_to_dto` pattern.
///
/// `is_builtin` is derived from the profile name via `is_builtin_profile()`
/// — not stored in config. This is the single mapping point; both
/// `settings_snapshot` and `profile_create`/`profile_update` use it.
///
/// Task 3: Profile downgrade — provider_id / model fields are gone from
/// both `SubagentProfileConfig` and `ProfileDto`. The DTO now exposes only
/// behavior-template fields (tools / context_budget_tokens / timeout_seconds
/// / write_access). Provider/model binding is per-subagent and surfaced via
/// the subagent detail DTO, not the profile DTO.
fn profile_to_dto(name: &str, profile: &busytok_config::SubagentProfileConfig) -> ProfileDto {
    ProfileDto {
        id: name.to_string(),
        is_builtin: busytok_config::is_builtin_profile(name),
        tools: profile.tools.clone(),
        context_budget_tokens: profile.context_budget_tokens,
        timeout_seconds: profile.timeout_seconds,
        write_access: profile.write_access,
    }
}

/// Try to spawn a background task that periodically reloads the price catalog.
///
/// Returns `None` when no Tokio runtime is active (safe for sync contexts).
/// Follows the same pattern as `writer::try_spawn_writer`.
fn try_spawn_catalog_reloader(catalog_path: PathBuf) -> Option<tokio::task::JoinHandle<()>> {
    tokio::runtime::Handle::try_current().ok().map(|rt| {
        rt.spawn(async move {
            use busytok_pricing::ReloadResult;
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(300)).await;
                match busytok_pricing::try_reload_catalog(&catalog_path) {
                    ReloadResult::Reloaded { version } => {
                        info!(
                            event_code = "pricing.catalog_reload_reloaded",
                            version,
                            catalog_path = %catalog_path.display(),
                            "price catalog reloaded"
                        );
                    }
                    ReloadResult::Invalid { reason } => {
                        warn!(
                            event_code = "pricing.catalog_reload_invalid",
                            reason,
                            catalog_path = %catalog_path.display(),
                            "price catalog reload skipped: validation failed"
                        );
                    }
                    ReloadResult::ParseError { error } => {
                        warn!(
                            event_code = "pricing.catalog_reload_parse_error",
                            error,
                            catalog_path = %catalog_path.display(),
                            "price catalog reload skipped: parse error"
                        );
                    }
                    ReloadResult::IoError { error } => {
                        warn!(
                            event_code = "pricing.catalog_reload_io_error",
                            error,
                            catalog_path = %catalog_path.display(),
                            "price catalog reload skipped: IO error"
                        );
                    }
                    ReloadResult::Missing | ReloadResult::Unchanged => {}
                }
            }
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_bucket_range_returns_fixed_two_second_window() {
        assert_eq!(
            BusytokSupervisor::live_bucket_range(10_001, 6),
            (6_000, 12_000)
        );
    }

    #[test]
    fn densify_live_samples_zero_fills_missing_buckets() {
        let samples = BusytokSupervisor::densify_live_samples(
            2_000,
            10_000,
            vec![LiveSampleDto {
                bucket_start_ms: 6_000,
                tokens_per_sec: 150.0,
                cost_per_sec: Some(0.25),
                events_per_sec: 0.5,
            }],
        );

        assert_eq!(samples.len(), 4);
        assert_eq!(
            samples
                .iter()
                .map(|sample| sample.bucket_start_ms)
                .collect::<Vec<_>>(),
            vec![2_000, 4_000, 6_000, 8_000]
        );
        assert_eq!(samples[0].tokens_per_sec, 0.0);
        assert_eq!(samples[1].tokens_per_sec, 0.0);
        assert_eq!(samples[2].tokens_per_sec, 150.0);
        assert_eq!(samples[2].events_per_sec, 0.5);
        assert_eq!(samples[3].tokens_per_sec, 0.0);
    }

    /// Build an `ActivityListRow` with unified fields filled for a
    /// cache-heavy event (cache_read=990 of prompt_input_total=1000).
    fn activity_list_row_fixture() -> busytok_store::read_models::ActivityListRow {
        use busytok_store::read_models::ActivityListRow;
        ActivityListRow {
            id: "row-1".to_string(),
            happened_at_ms: 0,
            client_kind: "claude".to_string(),
            session_id: "session-1".to_string(),
            source_file_id: "src-1".to_string(),
            source_path: "src-1".to_string(),
            project_hash: None,
            project_path: None,
            model: Some("claude-3-5-sonnet".to_string()),
            total_tokens: 1000,
            input_tokens: 1000,
            cached_input_tokens: 990,
            prompt_input_total_tokens: 1000,
            prompt_input_non_cached_tokens: 10,
            cache_read_tokens: 990,
            cache_creation_tokens: 0,
            cost_usd: None,
            is_error: false,
        }
    }

    #[test]
    fn activity_item_rate_uses_unified_denominator() {
        let row = activity_list_row_fixture();
        let rate = BusytokSupervisor::list_cache_hit_rate(&row).expect("rate present");
        assert!(rate <= 1.0);
        assert!((rate - 0.99).abs() < 1e-9);
        // The unified helper drives the public DTO field as well.
        let dto = BusytokSupervisor::activity_item_from_read_row(&row);
        let dto_rate = dto.cache_hit_rate.expect("dto rate present");
        assert!((dto_rate - rate).abs() < 1e-12);
    }

    // ── provider_test_connection fallback decision helpers ───────────
    // The handler enforces HTTPS, which makes a plain-HTTP mock server
    // infeasible. These tests pin the fallback decision + status
    // interpretation logic directly (Spec §4).

    fn status(code: u16) -> reqwest::StatusCode {
        reqwest::StatusCode::from_u16(code).expect("valid status code")
    }

    #[test]
    fn provider_test_connection_fallback_triggers_only_for_absent_endpoint_codes() {
        // 404/405/501 mean the /models endpoint is absent or unsupported → fall back.
        assert!(models_probe_should_fallback(status(404)));
        assert!(models_probe_should_fallback(status(405)));
        assert!(models_probe_should_fallback(status(501)));
        // Reachable-but-failing endpoints do NOT fall back — the endpoint itself
        // responded, so a /chat/completions probe would not add signal.
        assert!(!models_probe_should_fallback(status(200)));
        assert!(!models_probe_should_fallback(status(401)));
        assert!(!models_probe_should_fallback(status(403)));
        assert!(!models_probe_should_fallback(status(429)));
        assert!(!models_probe_should_fallback(status(500)));
        assert!(!models_probe_should_fallback(status(502)));
        assert!(!models_probe_should_fallback(status(503)));
    }

    #[test]
    fn provider_test_connection_chat_probe_model_defaults_when_empty() {
        // Non-empty whitelist → first model.
        assert_eq!(
            chat_probe_model(&["deepseek-chat".to_string(), "other".to_string()]),
            "deepseek-chat"
        );
        // Empty whitelist → generic default (probe only checks reachability).
        assert_eq!(chat_probe_model(&[]), "gpt-3.5-turbo");
    }

    #[test]
    fn provider_test_connection_interpret_chat_probe_status() {
        // 2xx → success.
        assert_eq!(interpret_chat_probe(status(200)), (true, None));
        assert_eq!(interpret_chat_probe(status(204)), (true, None));
        // 401/403 → connection works but auth failed.
        assert_eq!(
            interpret_chat_probe(status(401)),
            (
                false,
                Some("connection works but authentication failed".to_string())
            )
        );
        assert_eq!(
            interpret_chat_probe(status(403)),
            (
                false,
                Some("connection works but authentication failed".to_string())
            )
        );
        // 404/405/501 → both probes failed (endpoint unsupported).
        assert_eq!(
            interpret_chat_probe(status(404)),
            (
                false,
                Some("provider does not support /models or /chat/completions".to_string())
            )
        );
        assert_eq!(
            interpret_chat_probe(status(501)),
            (
                false,
                Some("provider does not support /models or /chat/completions".to_string())
            )
        );
        // Other → generic HTTP status string. `StatusCode`'s Display includes the
        // canonical reason phrase (e.g. "500 Internal Server Error"), matching the
        // existing non-fallback path's `format!("HTTP {}", status)`. Assert on the
        // stable numeric prefix so the test doesn't bind to reason-phrase wording.
        let (ok, msg) = interpret_chat_probe(status(500));
        assert!(!ok);
        assert!(
            msg.as_deref().unwrap().starts_with("HTTP 500"),
            "expected an HTTP 500 message, got: {msg:?}"
        );
        let (ok, msg) = interpret_chat_probe(status(429));
        assert!(!ok);
        assert!(
            msg.as_deref().unwrap().starts_with("HTTP 429"),
            "expected an HTTP 429 message, got: {msg:?}"
        );
    }

    // ── prompt_*_to_row enum conversion helpers ───────────────────────

    #[test]
    fn prompt_action_to_row_maps_all_variants() {
        assert_eq!(
            prompt_action_to_row(PromptActionDto::OnlyCopy),
            busytok_store::PromptActionRow::Copy
        );
        assert_eq!(
            prompt_action_to_row(PromptActionDto::OnlyPaste),
            busytok_store::PromptActionRow::Paste
        );
        assert_eq!(
            prompt_action_to_row(PromptActionDto::CopyAndPaste),
            busytok_store::PromptActionRow::Paste
        );
    }

    #[test]
    fn prompt_sort_to_row_defaults_to_smart_and_maps_all_variants() {
        assert_eq!(
            prompt_sort_to_row(None),
            busytok_store::PromptSortRow::Smart
        );
        assert_eq!(
            prompt_sort_to_row(Some(PromptSortDto::Smart)),
            busytok_store::PromptSortRow::Smart
        );
        assert_eq!(
            prompt_sort_to_row(Some(PromptSortDto::RecentlyUsed)),
            busytok_store::PromptSortRow::RecentlyUsed
        );
        assert_eq!(
            prompt_sort_to_row(Some(PromptSortDto::MostUsed)),
            busytok_store::PromptSortRow::MostUsed
        );
        assert_eq!(
            prompt_sort_to_row(Some(PromptSortDto::RecentlyUpdated)),
            busytok_store::PromptSortRow::RecentlyUpdated
        );
        assert_eq!(
            prompt_sort_to_row(Some(PromptSortDto::Alphabetical)),
            busytok_store::PromptSortRow::Alphabetical
        );
        assert_eq!(
            prompt_sort_to_row(Some(PromptSortDto::PinnedFirst)),
            busytok_store::PromptSortRow::PinnedFirst
        );
    }

    #[test]
    fn prompt_use_surface_to_row_maps_both_variants() {
        assert_eq!(
            prompt_use_surface_to_row(PromptUseSurfaceDto::Overlay),
            busytok_store::PromptUseSurfaceRow::Overlay
        );
        assert_eq!(
            prompt_use_surface_to_row(PromptUseSurfaceDto::Page),
            busytok_store::PromptUseSurfaceRow::Page
        );
    }

    #[test]
    fn prompt_use_outcome_to_row_maps_all_variants() {
        assert_eq!(
            prompt_use_outcome_to_row(PromptUseOutcomeDto::Copy),
            busytok_store::PromptUseOutcomeRow::Copy
        );
        assert_eq!(
            prompt_use_outcome_to_row(PromptUseOutcomeDto::PasteAttempted),
            busytok_store::PromptUseOutcomeRow::PasteAttempted
        );
        assert_eq!(
            prompt_use_outcome_to_row(PromptUseOutcomeDto::PasteFellBackToCopy),
            busytok_store::PromptUseOutcomeRow::PasteFellBackToCopy
        );
    }

    #[test]
    fn prompt_use_failure_reason_to_row_maps_all_variants() {
        assert_eq!(
            prompt_use_failure_reason_to_row(PromptUseFailureReasonDto::PermissionMissing),
            busytok_store::PromptUseFailureReasonRow::PermissionMissing
        );
        assert_eq!(
            prompt_use_failure_reason_to_row(PromptUseFailureReasonDto::FocusLost),
            busytok_store::PromptUseFailureReasonRow::FocusLost
        );
        assert_eq!(
            prompt_use_failure_reason_to_row(PromptUseFailureReasonDto::InjectionFailed),
            busytok_store::PromptUseFailureReasonRow::InjectionFailed
        );
        assert_eq!(
            prompt_use_failure_reason_to_row(PromptUseFailureReasonDto::UnsupportedPlatform),
            busytok_store::PromptUseFailureReasonRow::UnsupportedPlatform
        );
    }

    #[test]
    fn prompt_list_query_to_row_applies_default_limit_when_none() {
        let row = prompt_list_query_to_row(PromptListQueryDto {
            query: Some("hello".to_string()),
            tag: Some("work".to_string()),
            sort: Some(PromptSortDto::MostUsed),
            limit: None,
        });
        assert_eq!(row.query.as_deref(), Some("hello"));
        assert_eq!(row.tag.as_deref(), Some("work"));
        assert_eq!(row.sort, busytok_store::PromptSortRow::MostUsed);
        assert_eq!(row.limit, PROMPT_LIST_DEFAULT_LIMIT);
    }

    #[test]
    fn prompt_list_query_to_row_preserves_explicit_limit() {
        let row = prompt_list_query_to_row(PromptListQueryDto {
            query: None,
            tag: None,
            sort: None,
            limit: Some(42),
        });
        assert_eq!(row.limit, 42);
        assert_eq!(row.sort, busytok_store::PromptSortRow::Smart);
    }

    // ── provider / catalog DTO conversion helpers ─────────────────────

    #[test]
    fn provider_to_dto_hides_api_key_and_reports_has_key() {
        let p = busytok_domain::Provider {
            id: "pid".to_string(),
            name: "Acme".to_string(),
            provider_kind: busytok_domain::ProviderKind::OpenAiCompatible,
            base_url: "https://api.acme.com".to_string(),
            enabled: true,
            api_key: Some("sk-secret".to_string()),
            created_at_ms: 100,
            updated_at_ms: 200,
        };
        let dto = provider_to_dto(&p);
        assert_eq!(dto.id, "pid");
        assert_eq!(dto.name, "Acme");
        assert!(dto.enabled);
        assert!(dto.has_api_key);
        assert_eq!(dto.base_url, "https://api.acme.com");
    }

    #[test]
    fn provider_to_dto_reports_no_key_when_absent() {
        let p = busytok_domain::Provider {
            id: "pid2".to_string(),
            name: "NoKey".to_string(),
            provider_kind: busytok_domain::ProviderKind::OpenAiCompatible,
            base_url: "https://api.nokey.com".to_string(),
            enabled: false,
            api_key: None,
            created_at_ms: 1,
            updated_at_ms: 2,
        };
        let dto = provider_to_dto(&p);
        assert!(!dto.has_api_key);
        assert!(!dto.enabled);
    }

    #[test]
    fn provider_summary_to_dto_maps_all_fields() {
        let s = busytok_domain::ProviderSummary {
            id: "sid".to_string(),
            name: "Summary".to_string(),
            provider_kind: busytok_domain::ProviderKind::OpenAiCompatible,
            base_url: "https://sum.com".to_string(),
            enabled: true,
            has_api_key: true,
            created_at_ms: 10,
            updated_at_ms: 20,
        };
        let dto = provider_summary_to_dto(&s);
        assert_eq!(dto.id, "sid");
        assert_eq!(dto.name, "Summary");
        assert!(dto.has_api_key);
        assert!(dto.enabled);
        assert_eq!(dto.created_at_ms, 10);
    }

    #[test]
    fn catalog_entry_to_dto_maps_all_fields() {
        let e = busytok_domain::ModelCatalogEntry {
            provider_id: "pid".to_string(),
            provider_name: "PName".to_string(),
            provider_kind: busytok_domain::ProviderKind::OpenAiCompatible,
            provider_enabled: true,
            model_db_id: "mdb".to_string(),
            model_id: "gpt-x".to_string(),
            model_enabled: false,
            tags: vec!["fast".to_string(), "cheap".to_string()],
            display_name: None,
            reasoning: false,
            context_window: None,
            max_tokens: None,
        };
        let dto = catalog_entry_to_dto(e.clone());
        assert_eq!(dto.provider_id, "pid");
        assert_eq!(dto.provider_name, "PName");
        assert!(dto.provider_enabled);
        assert_eq!(dto.model_db_id, "mdb");
        assert_eq!(dto.model_id, "gpt-x");
        assert!(!dto.model_enabled);
        assert_eq!(dto.tags, vec!["fast", "cheap"]);
    }

    #[test]
    fn catalog_entry_to_dto_ref_produces_same_output_as_owned() {
        let e = busytok_domain::ModelCatalogEntry {
            provider_id: "pid".to_string(),
            provider_name: "PName".to_string(),
            provider_kind: busytok_domain::ProviderKind::OpenAiCompatible,
            provider_enabled: true,
            model_db_id: "mdb".to_string(),
            model_id: "gpt-x".to_string(),
            model_enabled: true,
            tags: vec![],
            display_name: None,
            reasoning: false,
            context_window: None,
            max_tokens: None,
        };
        let owned = catalog_entry_to_dto(e.clone());
        let by_ref = catalog_entry_to_dto_ref(&e);
        assert_eq!(owned.provider_id, by_ref.provider_id);
        assert_eq!(owned.model_id, by_ref.model_id);
        assert_eq!(owned.tags, by_ref.tags);
    }

    // ── profile helpers ───────────────────────────────────────────────

    #[test]
    fn profile_to_dto_marks_builtin_profiles_and_forwards_fields() {
        let profile = busytok_config::SubagentProfileConfig {
            write_access: true,
            tools: vec!["read".to_string()],
            context_budget_tokens: 4000,
            timeout_seconds: 120,
        };
        // Built-in profile name (shipped by default_profiles).
        let dto = profile_to_dto("pi/search-cheap", &profile);
        assert!(dto.is_builtin);
        assert!(dto.write_access);
        assert_eq!(dto.tools, vec!["read"]);
        assert_eq!(dto.context_budget_tokens, 4000);
        assert_eq!(dto.timeout_seconds, 120);
    }

    #[test]
    fn profile_to_dto_marks_custom_profiles_as_non_builtin() {
        let profile = busytok_config::SubagentProfileConfig {
            write_access: false,
            tools: vec![],
            context_budget_tokens: 1000,
            timeout_seconds: 30,
        };
        let dto = profile_to_dto("my-custom-profile", &profile);
        assert!(!dto.is_builtin);
    }

    // ── to_store_exact_windows ────────────────────────────────────────

    #[test]
    fn to_store_exact_windows_maps_empty_and_non_empty() {
        assert!(to_store_exact_windows(&[]).is_empty());
        let windows = vec![
            range::TrendBucketWindow {
                start_ms: 0,
                end_ms: 1000,
                key: "w1".to_string(),
                is_current: false,
            },
            range::TrendBucketWindow {
                start_ms: 1000,
                end_ms: 2000,
                key: "w2".to_string(),
                is_current: true,
            },
        ];
        let out = to_store_exact_windows(&windows);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].key, "w1");
        assert_eq!(out[0].start_ms, 0);
        assert_eq!(out[0].end_ms, 1000);
        assert_eq!(out[1].key, "w2");
    }

    // ── aggregate_trend_bucket ────────────────────────────────────────

    #[test]
    fn aggregate_trend_bucket_sums_only_rows_in_range() {
        let bucket = range::TrendBucketWindow {
            start_ms: 1000,
            end_ms: 2000,
            key: "b1".to_string(),
            is_current: true,
        };
        let rows = vec![
            busytok_store::read_models::OverviewTrendBucketRow {
                key: "b1".to_string(),
                start_ms: 1000, // in range
                end_ms: 2000,
                tokens: 100,
                cost_usd: Some(0.5),
                event_count: 2,
                has_cost: true,
                has_no_cost: false,
            },
            busytok_store::read_models::OverviewTrendBucketRow {
                key: "b1".to_string(),
                start_ms: 1500, // in range
                end_ms: 2000,
                tokens: 50,
                cost_usd: None,
                event_count: 1,
                has_cost: false,
                has_no_cost: true,
            },
            busytok_store::read_models::OverviewTrendBucketRow {
                key: "b1".to_string(),
                start_ms: 2000, // out of range (end-exclusive)
                end_ms: 3000,
                tokens: 999,
                cost_usd: None,
                event_count: 99,
                has_cost: false,
                has_no_cost: false,
            },
        ];
        let dto = aggregate_trend_bucket(&bucket, &TrendBucketGranularityDto::Hour, &rows);
        assert_eq!(dto.tokens, 150);
        assert_eq!(dto.event_count, 3);
        assert_eq!(dto.cost_usd, Some(0.5));
        assert!(dto.is_current);
    }

    #[test]
    fn aggregate_trend_bucket_returns_zero_tokens_for_no_matching_rows() {
        let bucket = range::TrendBucketWindow {
            start_ms: 10_000,
            end_ms: 20_000,
            key: "empty".to_string(),
            is_current: false,
        };
        let rows: Vec<busytok_store::read_models::OverviewTrendBucketRow> = vec![];
        let dto = aggregate_trend_bucket(&bucket, &TrendBucketGranularityDto::Day, &rows);
        assert_eq!(dto.tokens, 0);
        assert_eq!(dto.event_count, 0);
        assert_eq!(dto.cost_usd, None);
        assert!(!dto.is_current);
    }

    // ── map_subagent_error ────────────────────────────────────────────

    #[test]
    fn map_subagent_error_wraps_error_with_stable_code() {
        let err = map_subagent_error(busytok_subagent::SubagentError::NotFound(
            "sa-1".to_string(),
        ));
        let msg = err.to_string();
        assert!(msg.contains("logical subagent not found: sa-1"));
    }

    #[test]
    fn map_subagent_error_preserves_disabled_variant() {
        let err = map_subagent_error(busytok_subagent::SubagentError::Disabled);
        assert!(err.to_string().contains("subagent feature is disabled"));
    }

    // ── delegate_request_from_dto / resolve_params_from_dto ───────────

    #[test]
    fn delegate_request_from_dto_forwards_all_fields() {
        let dto = busytok_protocol::dto::SubagentDelegateRequestDto {
            subagent_name: "sa".to_string(),
            subagent_id: Some("id-1".to_string()),
            cwd: "/tmp".to_string(),
            profile: "pi/search-cheap".to_string(),
            intent: Some("find bugs".to_string()),
            prompt: "do thing".to_string(),
            prompt_artifact_ref: Some("art/123".to_string()),
            timeout_seconds: Some(60),
            model_override: Some("gpt-5".to_string()),
            source_harness: Some("gui".to_string()),
            source_session_id: Some("sess-1".to_string()),
            bound_provider_id: Some("prov-1".to_string()),
            bound_model_id: Some("gpt-5".to_string()),
        };
        let req = delegate_request_from_dto(dto);
        assert_eq!(req.subagent_name, "sa");
        assert_eq!(req.subagent_id.as_deref(), Some("id-1"));
        assert_eq!(req.cwd, "/tmp");
        assert_eq!(req.profile, "pi/search-cheap");
        assert_eq!(req.intent.as_deref(), Some("find bugs"));
        assert_eq!(req.prompt, "do thing");
        assert_eq!(req.prompt_artifact_ref.as_deref(), Some("art/123"));
        assert_eq!(req.timeout_seconds, Some(60));
        assert_eq!(req.model_override.as_deref(), Some("gpt-5"));
        assert_eq!(req.source_harness.as_deref(), Some("gui"));
        assert_eq!(req.source_session_id.as_deref(), Some("sess-1"));
        assert_eq!(req.bound_provider_id.as_deref(), Some("prov-1"));
        assert_eq!(req.bound_model_id.as_deref(), Some("gpt-5"));
    }

    #[test]
    fn resolve_params_from_dto_forwards_all_fields() {
        let dto = busytok_protocol::dto::SubagentResolveRequestDto {
            name: Some("sa".to_string()),
            id: Some("id-1".to_string()),
            cwd: Some("/tmp".to_string()),
        };
        let p = resolve_params_from_dto(dto);
        assert_eq!(p.name.as_deref(), Some("sa"));
        assert_eq!(p.id.as_deref(), Some("id-1"));
        assert_eq!(p.cwd.as_deref(), Some("/tmp"));
    }

    // ── subagent_detail / subagent_task_summary ───────────────────────

    #[test]
    fn subagent_detail_maps_status_to_string_and_forwards_fields() {
        let s = busytok_subagent::models::LogicalSubagent {
            id: "id-1".to_string(),
            name: "sa".to_string(),
            project_id: "proj".to_string(),
            repo_path: "/repo".to_string(),
            repo_hash: "hash".to_string(),
            branch: Some("main".to_string()),
            intent: Some("fix".to_string()),
            default_profile: "pi/search-cheap".to_string(),
            bound_provider_id: "prov-1".to_string(),
            bound_model_id: "gpt-5".to_string(),
            status: busytok_subagent::models::SubagentStatus::Hot,
            created_at_ms: 100,
            updated_at_ms: 200,
            last_active_at_ms: Some(150),
        };
        let dto = subagent_detail(s);
        assert_eq!(dto.id, "id-1");
        assert_eq!(dto.name, "sa");
        assert_eq!(dto.status, "hot");
        assert_eq!(dto.default_profile, "pi/search-cheap");
        assert_eq!(dto.bound_provider_id, "prov-1");
        assert_eq!(dto.bound_model_id, "gpt-5");
        assert_eq!(dto.last_active_at_ms, Some(150));
    }

    #[test]
    fn subagent_task_summary_maps_status_to_string_and_forwards_fields() {
        let t = busytok_subagent::models::SubagentTaskSummary {
            id: "task-1".to_string(),
            subagent_id: "sa-1".to_string(),
            profile: "pi/search-cheap".to_string(),
            status: busytok_subagent::models::TaskStatus::Completed,
            prompt: Some("prompt".to_string()),
            result_summary: Some("ok".to_string()),
            error: None,
            created_at_ms: 100,
            completed_at_ms: Some(200),
        };
        let dto = subagent_task_summary(t);
        assert_eq!(dto.id, "task-1");
        assert_eq!(dto.subagent_id, "sa-1");
        assert_eq!(dto.status, "completed");
        assert_eq!(dto.result_summary.as_deref(), Some("ok"));
        assert!(dto.error.is_none());
        assert_eq!(dto.completed_at_ms, Some(200));
    }

    // ── validate_runtime_dir ──────────────────────────────────────────

    #[test]
    fn validate_runtime_dir_rejects_relative_path() {
        let err = validate_runtime_dir("relative/path", "system").unwrap_err();
        assert!(err.to_string().contains("must be absolute"));
    }

    #[test]
    fn validate_runtime_dir_rejects_nonexistent_directory() {
        let err =
            validate_runtime_dir("/nonexistent/dir/that/does/not/exist", "system").unwrap_err();
        assert!(err
            .to_string()
            .contains("does not exist or is not a directory"));
    }

    #[test]
    fn validate_runtime_dir_rejects_dir_missing_bundle_and_manifest() {
        let tmp = std::env::temp_dir().join("busytok-validate-test-empty");
        std::fs::create_dir_all(&tmp).unwrap();
        let err = validate_runtime_dir(tmp.to_str().unwrap(), "system").unwrap_err();
        assert!(err.to_string().contains("pi-sidecar.bundle.js"));
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn validate_runtime_dir_rejects_dir_with_bundle_but_no_manifest() {
        let tmp = std::env::temp_dir().join("busytok-validate-test-bundle-only");
        std::fs::create_dir_all(&tmp).unwrap();
        std::fs::write(tmp.join("pi-sidecar.bundle.js"), "code").unwrap();
        let err = validate_runtime_dir(tmp.to_str().unwrap(), "system").unwrap_err();
        assert!(err.to_string().contains("manifest.json"));
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn validate_runtime_dir_rejects_malformed_manifest() {
        let tmp = std::env::temp_dir().join("busytok-validate-test-bad-manifest");
        std::fs::create_dir_all(&tmp).unwrap();
        std::fs::write(tmp.join("pi-sidecar.bundle.js"), "code").unwrap();
        std::fs::write(tmp.join("manifest.json"), "{ not valid json }").unwrap();
        let err = validate_runtime_dir(tmp.to_str().unwrap(), "system").unwrap_err();
        assert!(err.to_string().contains("not a valid SidecarManifest"));
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn validate_runtime_dir_rejects_manifest_bundle_drift() {
        let tmp = std::env::temp_dir().join("busytok-validate-test-drift");
        std::fs::create_dir_all(&tmp).unwrap();
        std::fs::write(tmp.join("pi-sidecar.bundle.js"), "code").unwrap();
        // Manifest references a different bundle filename than what's on disk.
        std::fs::write(
            tmp.join("manifest.json"),
            r#"{"version":"1","protocol_version":1,"bundle":"other.bundle.js","node_runtime_version":"22.6.0"}"#,
        )
        .unwrap();
        let err = validate_runtime_dir(tmp.to_str().unwrap(), "system").unwrap_err();
        assert!(err.to_string().contains("other.bundle.js"));
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn validate_runtime_dir_accepts_valid_system_runtime() {
        let tmp = std::env::temp_dir().join("busytok-validate-test-valid-system");
        std::fs::create_dir_all(&tmp).unwrap();
        std::fs::write(tmp.join("pi-sidecar.bundle.js"), "code").unwrap();
        std::fs::write(
            tmp.join("manifest.json"),
            r#"{"version":"1","protocol_version":1,"bundle":"pi-sidecar.bundle.js","node_runtime_version":"22.6.0"}"#,
        )
        .unwrap();
        let result = validate_runtime_dir(tmp.to_str().unwrap(), "system");
        assert!(result.is_ok(), "valid system runtime should pass");
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn validate_runtime_dir_rejects_bundled_node_missing_binary() {
        let tmp = std::env::temp_dir().join("busytok-validate-test-no-node");
        std::fs::create_dir_all(&tmp).unwrap();
        std::fs::write(tmp.join("pi-sidecar.bundle.js"), "code").unwrap();
        std::fs::write(
            tmp.join("manifest.json"),
            r#"{"version":"1","protocol_version":1,"bundle":"pi-sidecar.bundle.js","node_runtime_version":"22.6.0"}"#,
        )
        .unwrap();
        let err = validate_runtime_dir(tmp.to_str().unwrap(), "bundled").unwrap_err();
        assert!(err.to_string().contains("node"));
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn validate_runtime_dir_accepts_bundled_node_with_executable_binary() {
        let tmp = std::env::temp_dir().join("busytok-validate-test-bundled-ok");
        let node_dir = tmp.join("node").join(std::env::consts::ARCH);
        std::fs::create_dir_all(&node_dir).unwrap();
        std::fs::write(tmp.join("pi-sidecar.bundle.js"), "code").unwrap();
        std::fs::write(
            tmp.join("manifest.json"),
            r#"{"version":"1","protocol_version":1,"bundle":"pi-sidecar.bundle.js","node_runtime_version":"22.6.0"}"#,
        )
        .unwrap();
        // Write a node binary and mark it executable.
        let node_path = node_dir.join("node");
        std::fs::write(&node_path, "#!/bin/sh\nexit 0\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&node_path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let result = validate_runtime_dir(tmp.to_str().unwrap(), "bundled");
        assert!(
            result.is_ok(),
            "bundled runtime with executable node should pass"
        );
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn validate_runtime_dir_rejects_bundled_node_non_executable_binary() {
        let tmp = std::env::temp_dir().join("busytok-validate-test-bundled-noexec");
        let node_dir = tmp.join("node").join(std::env::consts::ARCH);
        std::fs::create_dir_all(&node_dir).unwrap();
        std::fs::write(tmp.join("pi-sidecar.bundle.js"), "code").unwrap();
        std::fs::write(
            tmp.join("manifest.json"),
            r#"{"version":"1","protocol_version":1,"bundle":"pi-sidecar.bundle.js","node_runtime_version":"22.6.0"}"#,
        )
        .unwrap();
        let node_path = node_dir.join("node");
        std::fs::write(&node_path, "not executable").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&node_path, std::fs::Permissions::from_mode(0o644)).unwrap();
        }
        let err = validate_runtime_dir(tmp.to_str().unwrap(), "bundled").unwrap_err();
        assert!(err.to_string().contains("not executable"));
        std::fs::remove_dir_all(&tmp).unwrap();
    }
}
