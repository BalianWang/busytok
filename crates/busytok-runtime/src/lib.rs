#![allow(warnings, clippy::all)]
#![allow(unexpected_cfgs)]
#![allow(dead_code)]
#![cfg_attr(test, allow(clippy::all))]
//! Busytok runtime: the central orchestration crate that wires
//! discovery → tailer → adapter → aggregator → store into a
//! complete scan pipeline, with cost enrichment from pricing.
//!
//! This crate does NOT do any proxy work. It reads local log files
//! from AI coding agents, parses them, and stores usage data in SQLite.

pub mod aggregates;
pub(crate) mod bootstrap;
pub(crate) mod generation_manager;
pub mod queue;
pub mod range;
pub mod read_service;
pub mod rebuild;
pub mod receipt;
pub mod sampler;
pub mod scan;
pub mod service_app;
pub(crate) mod source_registry;
pub mod status;
/// Phase 3 Task 5: usage normalization bridge for subagent tasks.
pub mod subagent_usage;
pub mod supervisor;
pub mod tail;
pub mod ui_models;
pub mod writer;

// Re-export key types for convenience.
#[doc(hidden)]
pub use queue::FileScanResult;
pub use queue::{ScanStats, TailWorkItem};
pub use scan::{derive_file_id, enrich_cost, scan_once};
pub use service_app::ServiceApp;
pub use supervisor::BusytokSupervisor;
pub use tail::start_tailing;

pub const CRATE_NAME: &str = env!("CARGO_PKG_NAME");
