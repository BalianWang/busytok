pub mod client;
pub mod protocol;

pub use client::SidecarRpcClient;
pub use protocol::*;

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
    Application(i32, String),
}
