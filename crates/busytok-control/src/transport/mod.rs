//! Platform-dispatched transport layer for the control IPC.
//!
//! `ControlTransport` is intentionally minimal: bind / accept / connect.
//! All frame encoding, handshake, RPC dispatch, and subscription gap
//! recovery live in platform-agnostic code that operates on `Self::Stream`
//! via `AsyncRead + AsyncWrite` bounds.

use anyhow::Result;
use async_trait::async_trait;
use tokio::io::{AsyncRead, AsyncWrite};

/// Platform transport contract.
///
/// `#[async_trait]` is required: native `async fn` in a public trait
/// triggers `async_fn_in_trait` lint (warn-by-default), and CI runs
/// `cargo clippy -- -D warnings`.
#[async_trait]
pub trait ControlTransport: 'static {
    type Listener: Send + Sync + 'static;
    type Stream: AsyncRead + AsyncWrite + Unpin + Send + 'static;

    async fn bind(endpoint: &str) -> Result<Self::Listener>;
    async fn accept(listener: &Self::Listener) -> Result<Self::Stream>;
    async fn connect(endpoint: &str) -> Result<Self::Stream>;
}

#[cfg(unix)]
pub mod unix;
#[cfg(windows)]
pub mod windows;

// Always available — used by both unit tests and the protocol-over-in-memory
// integration test in `tests/protocol_over_in_memory.rs` (which can only see
// items that are part of the normal build, not `#[cfg(test)]` items).
pub mod in_memory;

#[cfg(unix)]
pub type PlatformTransport = unix::UnixTransport;
#[cfg(windows)]
pub type PlatformTransport = windows::NamedPipeTransport;
#[cfg(not(any(unix, windows)))]
compile_error!("ControlTransport requires Unix or Windows target");
