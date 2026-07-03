//! Usage normalization bridge for the logical-subagent layer (Task 5).
//!
//! The subagent manager persists task usage into a private
//! `subagent_usage_records` table (internal bookkeeping). This module bridges
//! those raw `TaskUsage` values into the unified `usage_events` pipeline so
//! that subagent tokens appear in the Overview / Activity / receipt read
//! paths alongside top-level Codex and Claude Code events.
//!
//! The bridge lives in `busytok-runtime` (not `busytok-subagent`) because:
//! - The runtime owns the `GenerationManager` → can stamp the active
//!   `generation_id` (P0 fix — events written without the active
//!   generation_id are invisible to Overview/Activity read paths).
//! - The runtime owns the rollup infrastructure (`build_scan_mutations`) →
//!   can produce REAL `daily_usage` + `model_summary` rows (P1a fix —
//!   `RollupRows::default()` would make subagent tokens invisible to the
//!   heatmap / receipt panels).
//! - The runtime already depends on `busytok-pricing` → can compute
//!   `cost_usd` via the global `PriceCatalog` without adding a new
//!   dependency to `busytok-subagent`.
//!
//! `normalize_task_usage` is a pure function — the write itself is performed
//! by the `subagent_delegate` handler in `supervisor.rs` via
//! `db.ingest_store_batch(...)`.

use busytok_domain::{AgentKind, NormalizedUsageEvent};
use busytok_pricing::{CostMode, PriceCatalog, TokenUsage};
use busytok_subagent::models::TaskUsage;

/// Normalize a subagent task's `TaskUsage` into a `NormalizedUsageEvent`
/// suitable for the unified `usage_events` pipeline.
///
/// - `task_id` — the subagent task ID (used for event identity + dedupe).
/// - `subagent_id` — the logical subagent UUID (stored as `session_id`).
/// - `cwd` — the working directory the task ran in (stored as `cwd`).
/// - `usage` — the raw `TaskUsage` returned by the executor.
/// - `catalog` — the global `PriceCatalog` snapshot (may be `None` in tests
///   or when pricing is unavailable). When `None`, `cost_usd` is left
///   unset and `cost_source` is `None`.
///
/// The returned event uses `client_kind = "subagent"` as the discriminator
/// (so downstream consumers like the Activity page can distinguish subagent
/// events from top-level Codex runs), and `AgentKind::Codex` as the agent
/// (the pi-sidecar wraps a Codex-family SDK). The `dedupe_key` is
/// `subagent_task:{task_id}` so that re-writes for the same task collapse
/// onto a single row (idempotency).
pub fn normalize_task_usage(
    task_id: &str,
    subagent_id: &str,
    cwd: &str,
    usage: &TaskUsage,
    catalog: Option<&PriceCatalog>,
) -> NormalizedUsageEvent {
    let input = usage.input_tokens.unwrap_or(0).max(0) as u64;
    let output = usage.output_tokens.unwrap_or(0).max(0) as u64;
    let total = input + output;
    let model = usage.model.clone().unwrap_or_default();

    // Compute cost via the price catalog when available.
    // `estimate_cost_with_catalog` is a free function in `busytok_pricing`
    // (NOT a method on `PriceCatalog`). `TokenUsage` does not derive
    // `Default`, so all 5 fields must be spelled out.
    let cost_usd = catalog.and_then(|cat| {
        busytok_pricing::estimate_cost_with_catalog(
            cat,
            &model,
            TokenUsage {
                input_tokens: input,
                output_tokens: output,
                cached_input_tokens: 0,
                cache_creation_tokens: 0,
                reasoning_tokens: 0,
            },
            usage.cost_usd, // source_cost from sidecar (may be None)
            None,           // speed
            CostMode::Auto,
        )
    });

    // `NormalizedUsageEvent` has no `Default` impl. Use `minimal_for_test`
    // as the canonical zero-default constructor, then override fields.
    // `AgentKind` has no `Subagent` variant — use `Codex` (pi-sidecar wraps
    // a Codex-family SDK); `client_kind = "subagent"` is the discriminator.
    let event_id = format!("subagent_usage_{}", task_id);
    let mut event = NormalizedUsageEvent::minimal_for_test(&event_id, AgentKind::Codex);
    event.client_kind = Some("subagent".to_string());
    event.model = if model.is_empty() {
        None
    } else {
        Some(model.clone())
    };
    event.model_provider = usage.provider.clone();
    event.input_tokens = input as i64;
    event.output_tokens = output as i64;
    event.total_tokens = total as i64;
    event.cost_usd = cost_usd;
    event.estimated_cost_usd = cost_usd;
    event.cost_source = cost_usd.map(|_| "price_catalog".to_string());
    event.cwd = Some(cwd.to_string());
    event.session_id = subagent_id.to_string();
    event.dedupe_key = Some(format!("subagent_task:{}", task_id));
    // The dedupe_key already provides idempotency, so `raw_event_hash` just
    // needs to be a stable identifier for the event payload (no md5 needed).
    event.raw_event_hash = format!("subagent:{task_id}:{input}:{output}");
    event.timestamp_ms = busytok_domain::now_ms();
    event
}

#[cfg(test)]
mod tests {
    use super::*;
    use busytok_pricing::{ModelPrice, PriceTier, TierMode};
    use std::collections::HashMap;

    fn make_catalog(model: &str, input_rate: f64, output_rate: f64) -> PriceCatalog {
        PriceCatalog {
            schema_version: "3".to_string(),
            version: "test".to_string(),
            updated: "test".to_string(),
            aliases: HashMap::new(),
            prices: vec![ModelPrice {
                provider: "test".to_string(),
                model: model.to_string(),
                currency: "USD".to_string(),
                effective_date: "2099-01-01".to_string(),
                fast_multiplier: None,
                tier_mode: TierMode::Marginal,
                tiers: vec![PriceTier {
                    from_tokens: 0,
                    input_per_million: input_rate,
                    output_per_million: output_rate,
                    cached_input_per_million: None,
                    cache_write_per_million: None,
                    cache_storage_per_million_hour: None,
                    reasoning_per_million: None,
                }],
            }],
        }
    }

    fn make_usage(
        model: Option<&str>,
        input: Option<i64>,
        output: Option<i64>,
        cost: Option<f64>,
    ) -> TaskUsage {
        TaskUsage {
            model: model.map(|s| s.to_string()),
            provider: Some("test-provider".to_string()),
            input_tokens: input,
            output_tokens: output,
            cache_read_tokens: None,
            cache_write_tokens: None,
            cost_usd: cost,
        }
    }

    #[test]
    fn normalize_usage_populates_required_fields() {
        let usage = make_usage(Some("gpt-5"), Some(100), Some(200), None);
        let event = normalize_task_usage("task-1", "sub-1", "/repo", &usage, None);
        assert_eq!(event.client_kind.as_deref(), Some("subagent"));
        assert_eq!(event.model.as_deref(), Some("gpt-5"));
        assert_eq!(event.input_tokens, 100);
        assert_eq!(event.output_tokens, 200);
        assert_eq!(event.total_tokens, 300);
        assert_eq!(event.cwd.as_deref(), Some("/repo"));
        assert_eq!(event.session_id, "sub-1");
        assert_eq!(event.agent, AgentKind::Codex);
    }

    #[test]
    fn normalize_usage_handles_missing_tokens() {
        let usage = make_usage(Some("gpt-5"), None, None, None);
        let event = normalize_task_usage("task-2", "sub-1", "/repo", &usage, None);
        assert_eq!(event.input_tokens, 0);
        assert_eq!(event.output_tokens, 0);
        assert_eq!(event.total_tokens, 0);
    }

    #[test]
    fn normalize_usage_computes_cost_via_price_catalog() {
        let catalog = make_catalog("gpt-5", 1.25, 10.0);
        let usage = make_usage(Some("gpt-5"), Some(100_000), Some(50_000), None);
        let event = normalize_task_usage("task-3", "sub-1", "/repo", &usage, Some(&catalog));
        let cost = event.cost_usd.expect("cost should be computed");
        let expected = 100_000.0 * 1.25 / 1_000_000.0 + 50_000.0 * 10.0 / 1_000_000.0;
        assert!(
            (cost - expected).abs() < 0.0001,
            "expected {expected}, got {cost}"
        );
        assert_eq!(event.cost_source.as_deref(), Some("price_catalog"));
    }

    #[test]
    fn normalize_usage_cost_none_when_catalog_misses() {
        let catalog = make_catalog("other-model", 1.25, 10.0);
        let usage = make_usage(Some("gpt-5"), Some(100), Some(200), None);
        let event = normalize_task_usage("task-4", "sub-1", "/repo", &usage, Some(&catalog));
        assert!(
            event.cost_usd.is_none(),
            "cost should be None when model not in catalog"
        );
        assert!(event.cost_source.is_none());
    }

    #[test]
    fn normalize_usage_cost_none_when_no_catalog() {
        let usage = make_usage(Some("gpt-5"), Some(100), Some(200), None);
        let event = normalize_task_usage("task-5", "sub-1", "/repo", &usage, None);
        assert!(event.cost_usd.is_none());
        assert!(event.cost_source.is_none());
    }

    #[test]
    fn normalize_usage_prefers_source_cost_when_present() {
        // CostMode::Auto prefers source_cost_usd when present.
        let catalog = make_catalog("gpt-5", 1.25, 10.0);
        let usage = make_usage(Some("gpt-5"), Some(100_000), Some(50_000), Some(0.42));
        let event = normalize_task_usage("task-6", "sub-1", "/repo", &usage, Some(&catalog));
        assert_eq!(event.cost_usd, Some(0.42));
    }

    #[test]
    fn normalize_usage_dedupe_key_is_stable_per_task() {
        let usage = make_usage(Some("gpt-5"), Some(100), Some(200), None);
        let event = normalize_task_usage("task-7", "sub-1", "/repo", &usage, None);
        assert_eq!(event.dedupe_key.as_deref(), Some("subagent_task:task-7"));
    }

    #[test]
    fn normalize_usage_raw_event_hash_is_stable() {
        let usage = make_usage(Some("gpt-5"), Some(100), Some(200), None);
        let event = normalize_task_usage("task-8", "sub-1", "/repo", &usage, None);
        assert_eq!(event.raw_event_hash, "subagent:task-8:100:200");
    }

    /// When the executor reports no model, `event.model` must be `None`
    /// (NOT `Some("")`) so `build_scan_mutations` can skip it cleanly for
    /// `model_summary` aggregation — an empty-string model would either be
    /// skipped (wasting the row) or pollute the summary with a blank key.
    #[test]
    fn normalize_usage_model_none_when_absent_or_empty() {
        let no_model = TaskUsage {
            model: None,
            provider: None,
            input_tokens: Some(10),
            output_tokens: Some(20),
            cache_read_tokens: None,
            cache_write_tokens: None,
            cost_usd: None,
        };
        let event = normalize_task_usage("task-none", "sub-1", "/repo", &no_model, None);
        assert!(
            event.model.is_none(),
            "absent model must yield event.model=None, got {:?}",
            event.model
        );

        let empty_model = TaskUsage {
            model: Some(String::new()),
            provider: None,
            input_tokens: Some(10),
            output_tokens: Some(20),
            cache_read_tokens: None,
            cache_write_tokens: None,
            cost_usd: None,
        };
        let event = normalize_task_usage("task-empty", "sub-1", "/repo", &empty_model, None);
        assert!(
            event.model.is_none(),
            "empty model must yield event.model=None, got {:?}",
            event.model
        );
    }
}
