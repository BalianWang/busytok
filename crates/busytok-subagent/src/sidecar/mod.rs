pub mod client;
pub mod config;
pub mod executor;
pub mod pool;
pub mod protocol;
pub mod supervisor;

pub use client::SidecarRpcClient;
pub use config::{resolve_base_sidecar_config, resolve_sidecar_config, SidecarConfig};
pub use executor::SidecarTaskExecutor;
pub use pool::{ProviderRuntimeEntry, ResponderFactory, WorkerPool};
pub use protocol::*;
pub use supervisor::{
    PiSidecarSupervisor, PressureLevel, SharedDb, SidecarHandle, WorkerSnapshot, WorkerState,
};

/// Errors from sidecar operations.
#[derive(Debug, thiserror::Error)]
pub enum SidecarError {
    #[error("sidecar spawn failed: {0}")]
    Spawn(String),
    #[error("sidecar rpc error: {0}")]
    Rpc(String),
    #[error("sidecar timeout: {0}")]
    Timeout(String),
    #[error("sidecar crashed: {0}")]
    Crashed(String),
    #[error("sidecar io error: {0}")]
    Io(String),
    #[error("sidecar application error [{0}]: {1}")]
    Application(i32, String, Option<serde_json::Value>),
}
