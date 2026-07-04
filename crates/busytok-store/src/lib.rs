#![allow(warnings, clippy::all)]
#![allow(unexpected_cfgs)]
#![allow(dead_code)]
#![cfg_attr(test, allow(clippy::all))]
#![allow(
    clippy::too_many_arguments,
    clippy::collapsible_if,
    clippy::explicit_auto_deref,
    clippy::needless_borrow
)]
pub mod db;
pub mod generation_commands;
pub mod generation_queries;
pub mod live_queries;
pub mod outbox_queries;
pub mod prompt_entries;
pub mod provider_catalog;
pub mod read_models;
pub mod read_queries;
pub mod repository;
pub mod schema;
pub mod source_queries;
pub mod subagent_queries;
pub mod write_queries;

pub use db::{Database, IngestResult, OldEventTokens};
pub use prompt_entries::{
    NewPromptEntryRow, PromptActionRow, PromptEntryRow, PromptListQuery, PromptListResult,
    PromptSortRow, PromptUseFailureReasonRow, PromptUseOutcomeRow, PromptUseResultRow,
    PromptUseRow, PromptUseSurfaceRow, UpdatePromptEntryRow,
};
pub use provider_catalog::{
    CreateModelReq, CreateProviderReq, ModelCatalogEntry, ModelCatalogFilter, UpdateModelPatch,
    UpdateProviderPatch,
};
// Re-export domain types that store consumers need
pub use busytok_domain::{
    Model, ModelTag, ProfileModelRef, Provider, ProviderKind, ProviderSummary,
};
pub use read_models::DailyUsageTrendRow;
pub use repository::{
    CodexTokenSnapshotRow, DailyUsageRow, DiagnosticEventRow, LogFileRow, LogSourceRow,
    ModelSummaryRow, ModelUsageRow, ProjectRow, RealtimeSummaryRow, RollupRows, SessionRow,
    StoreHealthInfo, StoreWriteBatch, SubagentHarnessBindingRow, SubagentLogicalSubagentRow,
    SubagentMemoryRow, SubagentResourceEventRow, SubagentTaskRow, SubagentUsageRecordRow,
};
pub use subagent_queries::{CrashReconciliationCounts, ShutdownReconciliationCounts};

pub const CRATE_NAME: &str = env!("CARGO_PKG_NAME");
