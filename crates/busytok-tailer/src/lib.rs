#![allow(warnings, clippy::all)]
#![allow(unexpected_cfgs)]
#![allow(dead_code)]
#![cfg_attr(test, allow(clippy::all))]
#![allow(clippy::uninlined_format_args)]
//! Busytok tailer: reads JSONL log files, splits completed lines, and
//! tracks byte offsets for checkpointing.
//!
//! This crate is storage-agnostic and business-agnostic. It does NOT depend
//! on `busytok-store`, `busytok-aggregator`, or `busytok-adapters`. It only
//! reads bytes, splits completed JSONL lines, tracks exact byte offsets,
//! carries `inode` into `ParseContext` when the platform exposes it, and
//! returns a safe checkpoint candidate that excludes incomplete final lines.

pub mod scanner;
pub mod tailer;

// Re-export the primary scanner types for convenience.
pub use scanner::{
    read_file_once, read_inode, JsonlLineBuffer, ScanFileRequest, ScanReadBatch, TailedLine,
};

// Re-export the file watch service.
pub use tailer::{FileChangeEvent, FileChangeKind, FileWatchService};

pub const CRATE_NAME: &str = env!("CARGO_PKG_NAME");
