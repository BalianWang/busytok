//! Service bootstrap facade.
//!
//! Retains only the [`ServiceBootstrapStatus`] enum used by the GUI. The
//! actual `SMAppService` lifecycle logic lives in
//! [`crate::service_lifecycle::smappservice`]; callers reach it via the
//! [`crate::lifecycle_coordinator::LifecycleCoordinator`] held in Tauri
//! state, not through this module.

/// Status returned by the bootstrap path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServiceBootstrapStatus {
    AlreadyRunning,
    Started,
}
