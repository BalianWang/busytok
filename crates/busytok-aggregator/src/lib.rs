#![allow(warnings, clippy::all)]
#![allow(unexpected_cfgs)]
#![allow(dead_code)]
#![cfg_attr(test, allow(clippy::all))]
//! Busytok aggregator: deterministic aggregate computation for usage rollups.
//!
//! This crate owns aggregate computation logic: mutation planning (for
//! incremental scan batches) and full rollup rebuilds (for daily, session,
//! project, model summaries). It does NOT own the SQLite transaction -- that
//! belongs to the store. It does NOT parse log formats.

pub mod blocks;
pub mod daily;
pub mod mutations;
pub mod summary;

pub use blocks::{calculate_burn_rate, identify_session_blocks, DEFAULT_SESSION_DURATION_HOURS};
pub use daily::{build_weekly_usage_value, get_date_week};
pub use mutations::{
    build_scan_mutations, model_rollups_to_rows, project_rollups_to_rows, rebuild_model_summaries,
    rebuild_projects, rebuild_sessions, session_rollups_to_rows, RollupOptions,
    ScanAggregateMutations,
};
pub use summary::build_realtime_summary;

pub const CRATE_NAME: &str = env!("CARGO_PKG_NAME");
