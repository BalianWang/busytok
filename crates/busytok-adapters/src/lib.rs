#![allow(warnings, clippy::all)]
#![allow(unexpected_cfgs)]
#![allow(dead_code)]
#![cfg_attr(test, allow(clippy::all))]
//! Busytok adapters: parse agent-specific JSONL log formats into normalized events.
//!
//! The Claude Code adapter is the first fully supported adapter.
//! The Codex adapter emits cumulative token snapshots for runtime delta conversion.
//! Adapters must NOT read prompt/response content, tool arguments, or API keys
//! from the log lines.

pub mod adapter;
pub mod claude;
pub mod codex;

pub use adapter::AgentLogAdapter;
pub use claude::{calculate_context_from_transcript, ClaudeCodeAdapter, ContextWindowInfo};
pub use codex::CodexAdapter;
