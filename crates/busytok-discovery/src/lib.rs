//! Busytok discovery — finds local agent log directories and candidate files.
//!
//! This crate discovers where agent logs live on the local filesystem. It does
//! NOT read file contents — it only finds candidate files for downstream
//! processing (tailing, parsing, ingestion).
//!
//! # Design constraints
//!
//! - No proxy, auth, or network logic.
//! - No file content reading — only filesystem traversal.
//! - Deduplication by canonical path where possible.

pub mod claude;
pub mod codex;
pub mod source;

pub use claude::ClaudeCodeDiscovery;
pub use codex::CodexDiscovery;
pub use source::DiscoveredLogSource;

pub const CRATE_NAME: &str = env!("CARGO_PKG_NAME");
