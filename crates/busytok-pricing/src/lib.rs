#![allow(warnings, clippy::all)]
#![allow(unexpected_cfgs)]
#![allow(dead_code)]
#![cfg_attr(test, allow(clippy::all))]
#![allow(clippy::uninlined_format_args)]
#![cfg_attr(test, allow(clippy::inconsistent_digit_grouping))]
use std::path::Path;
use std::sync::atomic::Ordering;
use std::sync::{Arc, LazyLock};

use arc_swap::ArcSwap;
use serde::Deserialize;
use std::collections::HashMap;
use tracing::warn;

/// Token usage for a single model invocation.
#[derive(Debug, Clone, Copy)]
pub struct TokenUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cached_input_tokens: u64,
    pub cache_creation_tokens: u64,
    pub reasoning_tokens: u64,
}

/// Cost calculation mode — determines whether to trust agent-provided
/// cost or calculate from token counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CostMode {
    /// Prefer source_cost_usd when present, fall back to calculation.
    Auto,
    /// Always calculate from token counts, ignore source_cost_usd.
    Calculate,
    /// Always use source_cost_usd, return 0 when absent.
    Display,
}

/// Tier pricing mode — determines how tier boundaries are applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TierMode {
    /// Marginal: each token category is independently segmented across
    /// tier boundaries (Anthropic/OpenAI style).
    Marginal,
    /// Whole-request: a single tier is selected based on total prompt
    /// size, then all categories use that tier's rates (Google-style).
    WholeRequest,
}

impl Default for TierMode {
    fn default() -> Self {
        Self::Marginal
    }
}

/// Outcome of a catalog hot-reload attempt.
#[derive(Debug)]
pub enum ReloadResult {
    /// File metadata unchanged since last load.
    Unchanged,
    /// Catalog successfully reloaded from disk.
    Reloaded { version: String },
    /// File parsed but failed validation (e.g. empty prices).
    Invalid { reason: String },
    /// File could not be parsed as valid JSON.
    ParseError { error: String },
    /// I/O error reading the file (other than "not found").
    IoError { error: String },
    /// File does not exist.
    Missing,
}

/// The full price catalog.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PriceCatalog {
    pub schema_version: String,
    pub version: String,
    pub updated: String,
    pub aliases: HashMap<String, String>,
    pub prices: Vec<ModelPrice>,
}

impl PriceCatalog {
    /// Resolve a model name from agent logs to its catalog entry.
    ///
    /// Trims whitespace, looks up aliases, then exact-matches against
    /// `prices[].model`. Returns `None` if the model is unknown.
    pub fn resolve_model(&self, name: &str) -> Option<&ModelPrice> {
        let trimmed = name.trim();
        let resolved = self
            .aliases
            .get(trimmed)
            .map(|s| s.as_str())
            .unwrap_or(trimmed);
        self.prices.iter().find(|p| p.model == resolved)
    }
}

/// Per-model pricing entry.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelPrice {
    pub provider: String,
    pub model: String,
    pub currency: String,
    pub effective_date: String,
    pub fast_multiplier: Option<f64>,
    #[serde(default)]
    pub tier_mode: TierMode,
    pub tiers: Vec<PriceTier>,
}

/// A single tier in a model's pricing ladder.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PriceTier {
    pub from_tokens: u64,
    pub input_per_million: f64,
    pub output_per_million: f64,
    pub cached_input_per_million: Option<f64>,
    pub cache_write_per_million: Option<f64>,
    pub cache_storage_per_million_hour: Option<f64>,
    pub reasoning_per_million: Option<f64>,
}

// ---------------------------------------------------------------------------
// Global catalog state
// ---------------------------------------------------------------------------

static CATALOG: LazyLock<ArcSwap<PriceCatalog>> = LazyLock::new(|| {
    ArcSwap::from_pointee(
        parse_catalog_json(include_str!("price_catalog.json"))
            .expect("embedded price_catalog.json is valid"),
    )
});

static LAST_MTIME_NANOS: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(i64::MIN);
static LAST_FILE_LEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn validate_catalog(catalog: &PriceCatalog) -> Result<(), ReloadResult> {
    use std::collections::HashSet;

    if catalog.schema_version != "3" {
        return Err(ReloadResult::Invalid {
            reason: format!(
                "unsupported schema_version: '{}', expected '3'",
                catalog.schema_version
            ),
        });
    }
    if catalog.version.trim().is_empty() {
        return Err(ReloadResult::Invalid {
            reason: "catalog version is empty".to_string(),
        });
    }
    if catalog.prices.is_empty() {
        return Err(ReloadResult::Invalid {
            reason: "catalog has no prices".to_string(),
        });
    }

    // Check model uniqueness.
    let mut seen_models = HashSet::new();
    for (i, mp) in catalog.prices.iter().enumerate() {
        if !seen_models.insert(&mp.model) {
            return Err(ReloadResult::Invalid {
                reason: format!("duplicate model '{}' at index {}", mp.model, i),
            });
        }
    }

    // Check alias targets exist.
    for (alias, target) in &catalog.aliases {
        if !catalog.prices.iter().any(|p| &p.model == target) {
            return Err(ReloadResult::Invalid {
                reason: format!("alias '{}' targets unknown model '{}'", alias, target),
            });
        }
    }

    // Per-model validation.
    for mp in &catalog.prices {
        if mp.provider.trim().is_empty() {
            return Err(ReloadResult::Invalid {
                reason: format!("model '{}' has empty provider", mp.model),
            });
        }
        if !mp
            .provider
            .chars()
            .all(|c| c.is_ascii_lowercase() || c == '-' || c == '_')
        {
            return Err(ReloadResult::Invalid {
                reason: format!(
                    "model '{}' provider '{}' must be lowercase",
                    mp.model, mp.provider
                ),
            });
        }
        if mp.currency != "USD" {
            return Err(ReloadResult::Invalid {
                reason: format!(
                    "model '{}' currency must be 'USD', got '{}'",
                    mp.model, mp.currency
                ),
            });
        }
        if let Some(fm) = mp.fast_multiplier {
            if fm <= 0.0 || !fm.is_finite() {
                return Err(ReloadResult::Invalid {
                    reason: format!(
                        "model '{}' fast_multiplier must be > 0 and finite, got {}",
                        mp.model, fm
                    ),
                });
            }
        }

        if mp.tiers.is_empty() {
            return Err(ReloadResult::Invalid {
                reason: format!("model '{}' has empty tiers", mp.model),
            });
        }
        if mp.tiers[0].from_tokens != 0 {
            return Err(ReloadResult::Invalid {
                reason: format!(
                    "model '{}' first tier from_tokens must be 0, got {}",
                    mp.model, mp.tiers[0].from_tokens
                ),
            });
        }
        for w in mp.tiers.windows(2) {
            if w[1].from_tokens <= w[0].from_tokens {
                return Err(ReloadResult::Invalid {
                    reason: format!(
                        "model '{}' tiers not strictly increasing: {} then {}",
                        mp.model, w[0].from_tokens, w[1].from_tokens
                    ),
                });
            }
        }

        for (ti, tier) in mp.tiers.iter().enumerate() {
            fn check_required(
                name: &str,
                val: f64,
                model: &str,
                ti: usize,
            ) -> Result<(), ReloadResult> {
                if !val.is_finite() || val < 0.0 {
                    return Err(ReloadResult::Invalid {
                        reason: format!(
                            "model '{}' tier {} {} must be >= 0 and finite, got {}",
                            model, ti, name, val
                        ),
                    });
                }
                Ok(())
            }
            fn check_optional(
                name: &str,
                val: Option<f64>,
                model: &str,
                ti: usize,
            ) -> Result<(), ReloadResult> {
                if let Some(v) = val {
                    if !v.is_finite() || v < 0.0 {
                        return Err(ReloadResult::Invalid {
                            reason: format!(
                                "model '{}' tier {} {} must be >= 0 and finite if set, got {}",
                                model, ti, name, v
                            ),
                        });
                    }
                }
                Ok(())
            }
            check_required("input_per_million", tier.input_per_million, &mp.model, ti)?;
            check_required("output_per_million", tier.output_per_million, &mp.model, ti)?;
            check_optional(
                "cached_input_per_million",
                tier.cached_input_per_million,
                &mp.model,
                ti,
            )?;
            check_optional(
                "cache_write_per_million",
                tier.cache_write_per_million,
                &mp.model,
                ti,
            )?;
            check_optional(
                "cache_storage_per_million_hour",
                tier.cache_storage_per_million_hour,
                &mp.model,
                ti,
            )?;
            check_optional(
                "reasoning_per_million",
                tier.reasoning_per_million,
                &mp.model,
                ti,
            )?;
        }
    }

    Ok(())
}

fn parse_catalog_json(json: &str) -> Result<PriceCatalog, ReloadResult> {
    let catalog: PriceCatalog =
        serde_json::from_str(json).map_err(|e| ReloadResult::ParseError {
            error: e.to_string(),
        })?;
    validate_catalog(&catalog)?;
    Ok(catalog)
}

fn read_file_state(path: &Path) -> Option<(i64, u64)> {
    let metadata = std::fs::metadata(path).ok()?;
    let mtime = metadata.modified().ok()?;
    let mtime_nanos = mtime.duration_since(std::time::UNIX_EPOCH).ok()?.as_nanos() as i64;
    let file_len = metadata.len();
    Some((mtime_nanos, file_len))
}

fn load_catalog_from_file(path: &Path) -> std::io::Result<Option<String>> {
    match std::fs::read_to_string(path) {
        Ok(content) => Ok(Some(content)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Initialize the global catalog from an optional file path.
///
/// If a path is provided and the file exists + parses successfully, it is
/// used. Otherwise the embedded catalog is loaded. Always records file state
/// so subsequent `try_reload_catalog` calls can detect changes.
pub fn init_catalog(catalog_path: Option<&Path>) {
    if let Some(path) = catalog_path {
        match load_catalog_from_file(path) {
            Ok(Some(content)) => match parse_catalog_json(&content) {
                Ok(catalog) => {
                    if let Some((mtime_nanos, file_len)) = read_file_state(path) {
                        LAST_MTIME_NANOS.store(mtime_nanos, Ordering::SeqCst);
                        LAST_FILE_LEN.store(file_len, Ordering::SeqCst);
                    }
                    CATALOG.store(Arc::new(catalog));
                    return;
                }
                Err(e) => {
                    warn!(
                        event_code = "pricing.catalog_init_failed",
                        reason = ?e,
                        catalog_path = %path.display(),
                        "price catalog init failed, falling back to embedded"
                    );
                }
            },
            Ok(None) => {} // File doesn't exist — not an error.
            Err(e) => {
                warn!(
                    event_code = "pricing.catalog_init_failed",
                    reason = %e,
                    catalog_path = %path.display(),
                    "price catalog init failed, falling back to embedded"
                );
            }
        }
    }

    // Fall back to embedded catalog — always re-store explicitly so that a
    // previous `reset_catalog()` in tests does not leave the static empty.
    let catalog =
        parse_catalog_json(include_str!("price_catalog.json")).expect("embedded catalog is valid");
    LAST_MTIME_NANOS.store(i64::MIN, Ordering::SeqCst);
    LAST_FILE_LEN.store(0, Ordering::SeqCst);
    CATALOG.store(Arc::new(catalog));
}

/// Load the current global price catalog.
///
/// Returns an `Arc<PriceCatalog>` that auto-derefs, so callers can use it
/// exactly like a `&PriceCatalog`.
pub fn load_catalog() -> Arc<PriceCatalog> {
    CATALOG.load_full()
}

/// Attempt to hot-reload the catalog from `catalog_path`.
///
/// Compares file modification time and size against the last-known values.
/// Returns a `ReloadResult` describing what happened. On parse or validation
/// error the existing catalog is left untouched.
pub fn try_reload_catalog(catalog_path: &Path) -> ReloadResult {
    let Some((mtime_nanos, file_len)) = read_file_state(catalog_path) else {
        return ReloadResult::Missing;
    };

    let prev_mtime = LAST_MTIME_NANOS.load(Ordering::SeqCst);
    let prev_len = LAST_FILE_LEN.load(Ordering::SeqCst);

    if mtime_nanos == prev_mtime && file_len == prev_len {
        return ReloadResult::Unchanged;
    }

    let content = match load_catalog_from_file(catalog_path) {
        Ok(Some(c)) => c,
        Ok(None) => return ReloadResult::Missing,
        Err(e) => {
            return ReloadResult::IoError {
                error: e.to_string(),
            }
        }
    };

    let catalog = match parse_catalog_json(&content) {
        Ok(c) => c,
        Err(e) => return e,
    };

    let version = catalog.version.clone();
    CATALOG.store(Arc::new(catalog));
    LAST_MTIME_NANOS.store(mtime_nanos, Ordering::SeqCst);
    LAST_FILE_LEN.store(file_len, Ordering::SeqCst);
    ReloadResult::Reloaded { version }
}

/// Price a token category against the tiered pricing ladder.
///
/// Splits `tokens` by `tiers[].from_tokens` thresholds, prices each segment
/// using `selector`, and accumulates the total. If `selector` returns `None`
/// for a segment, the contribution is 0 — the caller decides whether to warn.
fn price_by_tiers(
    tokens: u64,
    tiers: &[PriceTier],
    selector: fn(&PriceTier) -> Option<f64>,
) -> f64 {
    if tokens == 0 {
        return 0.0;
    }
    let mut cost = 0.0_f64;
    for (i, tier) in tiers.iter().enumerate() {
        let tier_start = tier.from_tokens;
        let tier_end = tiers.get(i + 1).map(|t| t.from_tokens).unwrap_or(u64::MAX);
        if tokens <= tier_start {
            break;
        }
        let seg_end = tier_end.min(tokens);
        let in_tier = seg_end - tier_start;
        if let Some(rate) = selector(tier) {
            cost += in_tier as f64 * rate / 1_000_000.0;
        }
    }
    cost
}

/// Select the applicable tier for whole-request pricing.
///
/// Returns the tier with the largest `from_tokens <= total_input`.
fn select_whole_request_tier(total_input: u64, tiers: &[PriceTier]) -> &PriceTier {
    tiers
        .iter()
        .rev()
        .find(|t| t.from_tokens <= total_input)
        .expect("catalog validation guarantees at least one tier starting from_tokens=0")
}

/// Price a token category using whole-request tier selection.
///
/// Selects the tier with the largest `from_tokens <= total_input`, then
/// prices all `tokens` at that tier's rate. Returns 0 for null selectors.
fn price_whole_request(
    total_input: u64,
    tokens: u64,
    tiers: &[PriceTier],
    selector: fn(&PriceTier) -> Option<f64>,
) -> f64 {
    if tokens == 0 {
        return 0.0;
    }
    let tier = select_whole_request_tier(total_input, tiers);
    match selector(tier) {
        Some(rate) => tokens as f64 * rate / 1_000_000.0,
        None => 0.0,
    }
}

/// Price a token category, dispatching on tier mode.
/// `total_input` is unused for Marginal mode but accepted uniformly for caller simplicity.
fn price_category(
    tier_mode: TierMode,
    total_input: u64,
    tokens: u64,
    tiers: &[PriceTier],
    selector: fn(&PriceTier) -> Option<f64>,
) -> f64 {
    match tier_mode {
        TierMode::Marginal => price_by_tiers(tokens, tiers, selector),
        TierMode::WholeRequest => price_whole_request(total_input, tokens, tiers, selector),
    }
}

/// Whether to warn about null pricing for a token category.
///
/// For Marginal mode: warn if ALL tiers have null (no segment can be priced).
/// For WholeRequest mode: warn if the SELECTED tier has null.
fn should_warn_null_price(
    tier_mode: TierMode,
    total_input: u64,
    tiers: &[PriceTier],
    selector: fn(&PriceTier) -> Option<f64>,
) -> bool {
    match tier_mode {
        TierMode::Marginal => tiers.iter().all(|t| selector(t).is_none()),
        TierMode::WholeRequest => selector(select_whole_request_tier(total_input, tiers)).is_none(),
    }
}

/// Estimate cost using an explicit catalog reference.
///
/// Uses `catalog.resolve_model()` for alias-aware lookup, then prices each
/// token category through `price_category` which dispatches based on
/// tier thresholds.
pub fn estimate_cost_with_catalog(
    catalog: &PriceCatalog,
    model: &str,
    usage: TokenUsage,
    source_cost: Option<f64>,
    speed: Option<&str>,
    mode: CostMode,
) -> Option<f64> {
    match mode {
        CostMode::Display => return Some(source_cost.unwrap_or(0.0)),
        CostMode::Calculate => {}
        CostMode::Auto => {
            if let Some(cost) = source_cost {
                return Some(cost);
            }
        }
    }

    let Some(price) = catalog.resolve_model(model) else {
        let trimmed = model.trim();
        if !trimmed.is_empty() {
            warn!(
                event_code = "pricing.model_not_found",
                raw_model = model,
                "no price catalog entry found for model"
            );
        }
        return None;
    };

    // Token decomposition with clamping.
    // Invariant: cached + creation <= input_tokens
    let cached = usage.cached_input_tokens.min(usage.input_tokens);
    let creation = usage
        .cache_creation_tokens
        .min(usage.input_tokens.saturating_sub(cached));
    let non_cached = usage
        .input_tokens
        .saturating_sub(cached)
        .saturating_sub(creation);

    let mut cost = 0.0_f64;
    let tier_mode = price.tier_mode;
    // Tier selection basis: input_tokens (total prompt size).
    // Some providers tier on "prompts <= 200k tokens" — cached/creation are
    // subsets of input, so input_tokens alone represents total prompt size.
    let total_input = usage.input_tokens;

    // Non-cached input tokens
    cost += price_category(tier_mode, total_input, non_cached, &price.tiers, |t| {
        Some(t.input_per_million)
    });

    // Cache read tokens
    cost += price_category(tier_mode, total_input, cached, &price.tiers, |t| {
        t.cached_input_per_million
    });
    if cached > 0
        && should_warn_null_price(tier_mode, total_input, &price.tiers, |t| {
            t.cached_input_per_million
        })
    {
        warn!(
            event_code = "pricing.cache_read_unpriced",
            model = price.model,
            tier_mode = ?tier_mode,
            total_input,
            cached_tokens = cached,
            "cache read tokens present but no cached_input_per_million rate available; cost contribution is 0"
        );
    }

    // Cache write tokens
    cost += price_category(tier_mode, total_input, creation, &price.tiers, |t| {
        t.cache_write_per_million
    });
    if creation > 0
        && should_warn_null_price(tier_mode, total_input, &price.tiers, |t| {
            t.cache_write_per_million
        })
    {
        warn!(
            event_code = "pricing.cache_write_unpriced",
            model = price.model,
            tier_mode = ?tier_mode,
            total_input,
            cache_creation_tokens = creation,
            "cache write tokens present but no cache_write_per_million rate available; cost contribution is 0"
        );
    }

    // Output tokens
    cost += price_category(
        tier_mode,
        total_input,
        usage.output_tokens,
        &price.tiers,
        |t| Some(t.output_per_million),
    );

    // Reasoning tokens
    cost += price_category(
        tier_mode,
        total_input,
        usage.reasoning_tokens,
        &price.tiers,
        |t| t.reasoning_per_million,
    );
    if usage.reasoning_tokens > 0
        && should_warn_null_price(tier_mode, total_input, &price.tiers, |t| {
            t.reasoning_per_million
        })
    {
        warn!(
            event_code = "pricing.reasoning_unpriced",
            model = price.model,
            tier_mode = ?tier_mode,
            total_input,
            reasoning_tokens = usage.reasoning_tokens,
            "reasoning tokens present but model has null reasoning_per_million; cost contribution is 0 (reasoning may be bundled in output)"
        );
    }

    // Fast-mode multiplier
    if speed == Some("fast") {
        if let Some(multiplier) = price.fast_multiplier {
            cost *= multiplier;
        }
    }

    Some(cost)
}

/// Estimate the cost in USD for a model invocation.
///
/// Behaviour depends on `mode`:
/// - `CostMode::Display` — return `source_cost` or 0
/// - `CostMode::Calculate` — always compute from tokens, ignore source_cost
/// - `CostMode::Auto` — prefer source_cost, fall back to token calculation
///
/// When computing from tokens:
/// - `cached_input_tokens` is clamped to `input_tokens`.
/// - `cache_creation_tokens` is clamped to `input_tokens.saturating_sub(cached)`.
///   The invariant `cached + creation <= input_tokens` is always maintained.
/// - Reasoning tokens are informational only unless the catalog entry has a
///   non-null `reasoning_per_million`; they are never double-counted into
///   output cost.
/// - If `speed` is `"fast"` and the model has a `fast_multiplier`, the total
///   token-based cost is multiplied by that factor.
/// - Unknown models return `None` (do not block ingestion).
pub fn estimate_cost_usd(
    model: &str,
    usage: TokenUsage,
    source_cost: Option<f64>,
    speed: Option<&str>,
    mode: CostMode,
) -> Option<f64> {
    let catalog = load_catalog();
    estimate_cost_with_catalog(&catalog, model, usage, source_cost, speed, mode)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    // -- helpers --

    fn reset_catalog() {
        CATALOG.store(Arc::new(PriceCatalog {
            schema_version: "3".to_string(),
            version: String::new(),
            updated: String::new(),
            aliases: HashMap::new(),
            prices: vec![],
        }));
        LAST_MTIME_NANOS.store(i64::MIN, Ordering::SeqCst);
        LAST_FILE_LEN.store(0, Ordering::SeqCst);
    }

    fn write_file_with_distinct_mtime(path: &std::path::Path, content: &str) {
        let _ = std::fs::remove_file(path);
        std::fs::write(path, content).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(10));
    }

    fn make_single_tier_catalog(model: &str, input_rate: f64, output_rate: f64) -> PriceCatalog {
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

    fn make_two_tier_catalog() -> PriceCatalog {
        PriceCatalog {
            schema_version: "3".to_string(),
            version: "test".to_string(),
            updated: "test".to_string(),
            aliases: HashMap::from([(
                "sonnet".to_string(),
                "claude-sonnet-4-20250514".to_string(),
            )]),
            prices: vec![ModelPrice {
                provider: "anthropic".to_string(),
                model: "claude-sonnet-4-20250514".to_string(),
                currency: "USD".to_string(),
                effective_date: "2099-01-01".to_string(),
                fast_multiplier: Some(6.0),
                tier_mode: TierMode::Marginal,
                tiers: vec![
                    PriceTier {
                        from_tokens: 0,
                        input_per_million: 3.0,
                        output_per_million: 15.0,
                        cached_input_per_million: Some(0.3),
                        cache_write_per_million: Some(3.75),
                        cache_storage_per_million_hour: None,
                        reasoning_per_million: None,
                    },
                    PriceTier {
                        from_tokens: 200000,
                        input_per_million: 6.0,
                        output_per_million: 22.5,
                        cached_input_per_million: Some(0.6),
                        cache_write_per_million: Some(7.5),
                        cache_storage_per_million_hour: None,
                        reasoning_per_million: None,
                    },
                ],
            }],
        }
    }

    fn zero_usage() -> TokenUsage {
        TokenUsage {
            input_tokens: 0,
            output_tokens: 0,
            cached_input_tokens: 0,
            cache_creation_tokens: 0,
            reasoning_tokens: 0,
        }
    }

    // -- alias resolution tests --

    #[test]
    #[serial]
    fn resolve_model_trims_whitespace() {
        let catalog = make_single_tier_catalog("gpt-5", 1.0, 2.0);
        assert!(catalog.resolve_model("  gpt-5  ").is_some());
    }

    #[test]
    #[serial]
    fn resolve_model_via_alias() {
        let catalog = make_two_tier_catalog();
        let found = catalog.resolve_model("sonnet");
        assert!(found.is_some());
        assert_eq!(found.unwrap().model, "claude-sonnet-4-20250514");
    }

    #[test]
    #[serial]
    fn resolve_model_direct_match() {
        let catalog = make_two_tier_catalog();
        assert!(catalog.resolve_model("claude-sonnet-4-20250514").is_some());
    }

    #[test]
    #[serial]
    fn resolve_model_unknown_returns_none() {
        let catalog = make_single_tier_catalog("gpt-5", 1.0, 2.0);
        assert!(catalog.resolve_model("nonexistent").is_none());
    }

    #[test]
    #[serial]
    fn resolve_model_empty_aliases_direct_match_works() {
        let catalog = make_single_tier_catalog("gpt-5", 1.0, 2.0);
        assert!(catalog.aliases.is_empty());
        assert!(catalog.resolve_model("gpt-5").is_some());
    }

    // -- cost mode tests --

    #[test]
    #[serial]
    fn cost_mode_display_returns_source_or_zero() {
        let catalog = make_single_tier_catalog("gpt-5", 1.0, 2.0);
        assert_eq!(
            estimate_cost_with_catalog(
                &catalog,
                "gpt-5",
                zero_usage(),
                Some(1.23),
                None,
                CostMode::Display
            ),
            Some(1.23)
        );
        assert_eq!(
            estimate_cost_with_catalog(
                &catalog,
                "gpt-5",
                zero_usage(),
                None,
                None,
                CostMode::Display
            ),
            Some(0.0)
        );
    }

    #[test]
    #[serial]
    fn cost_mode_auto_prefers_source_cost() {
        let catalog = make_single_tier_catalog("gpt-5", 1.0, 2.0);
        let usage = TokenUsage {
            input_tokens: 1_000,
            output_tokens: 1_000,
            ..zero_usage()
        };
        let cost =
            estimate_cost_with_catalog(&catalog, "gpt-5", usage, Some(0.42), None, CostMode::Auto);
        assert_eq!(cost, Some(0.42));
    }

    #[test]
    #[serial]
    fn cost_mode_auto_falls_back_to_calculation() {
        let catalog = make_single_tier_catalog("gpt-5", 1.25, 10.0);
        let usage = TokenUsage {
            input_tokens: 100_000,
            output_tokens: 50_000,
            ..zero_usage()
        };
        let cost = estimate_cost_with_catalog(&catalog, "gpt-5", usage, None, None, CostMode::Auto)
            .unwrap();
        let expected = 100_000.0 * 1.25 / 1_000_000.0 + 50_000.0 * 10.0 / 1_000_000.0;
        assert!((cost - expected).abs() < 0.0001);
    }

    #[test]
    #[serial]
    fn cost_mode_calculate_ignores_source_cost() {
        let catalog = make_single_tier_catalog("gpt-5", 1.25, 10.0);
        let usage = TokenUsage {
            input_tokens: 100_000,
            output_tokens: 50_000,
            ..zero_usage()
        };
        let cost = estimate_cost_with_catalog(
            &catalog,
            "gpt-5",
            usage,
            Some(999.0),
            None,
            CostMode::Calculate,
        )
        .unwrap();
        let expected = 100_000.0 * 1.25 / 1_000_000.0 + 50_000.0 * 10.0 / 1_000_000.0;
        assert!((cost - expected).abs() < 0.0001);
        assert!((cost - 999.0).abs() > 0.01);
    }

    #[test]
    #[serial]
    fn unknown_model_returns_none() {
        let catalog = make_single_tier_catalog("gpt-5", 1.0, 2.0);
        let cost = estimate_cost_with_catalog(
            &catalog,
            "unknown",
            zero_usage(),
            None,
            None,
            CostMode::Auto,
        );
        assert!(cost.is_none());
    }

    #[test]
    #[serial]
    fn empty_model_returns_none() {
        let catalog = make_single_tier_catalog("gpt-5", 1.0, 2.0);
        let cost =
            estimate_cost_with_catalog(&catalog, "", zero_usage(), None, None, CostMode::Auto);
        assert!(cost.is_none());
    }

    // -- single-tier pricing tests --

    #[test]
    #[serial]
    fn single_tier_correct_cost() {
        let catalog = make_single_tier_catalog("test-model", 10.0, 20.0);
        let usage = TokenUsage {
            input_tokens: 100_000,
            output_tokens: 50_000,
            ..zero_usage()
        };
        let cost = estimate_cost_with_catalog(
            &catalog,
            "test-model",
            usage,
            None,
            None,
            CostMode::Calculate,
        )
        .unwrap();
        let expected = 100_000.0 * 10.0 / 1_000_000.0 + 50_000.0 * 20.0 / 1_000_000.0;
        assert!((cost - expected).abs() < 0.0001);
    }

    // -- two-tier pricing tests --

    #[test]
    #[serial]
    fn two_tier_200k_below_threshold() {
        let catalog = make_two_tier_catalog();
        let usage = TokenUsage {
            input_tokens: 100_000,
            output_tokens: 50_000,
            ..zero_usage()
        };
        let cost = estimate_cost_with_catalog(
            &catalog,
            "claude-sonnet-4-20250514",
            usage,
            None,
            None,
            CostMode::Calculate,
        )
        .unwrap();
        let expected = 100_000.0 * 3.0 / 1_000_000.0 + 50_000.0 * 15.0 / 1_000_000.0;
        assert!((cost - expected).abs() < 0.0001);
    }

    #[test]
    #[serial]
    fn two_tier_200k_above_threshold() {
        let catalog = make_two_tier_catalog();
        let usage = TokenUsage {
            input_tokens: 300_000,
            output_tokens: 0,
            ..zero_usage()
        };
        let cost = estimate_cost_with_catalog(
            &catalog,
            "claude-sonnet-4-20250514",
            usage,
            None,
            None,
            CostMode::Calculate,
        )
        .unwrap();
        let expected = 200_000.0 * 3.0 / 1_000_000.0 + 100_000.0 * 6.0 / 1_000_000.0;
        assert!((cost - expected).abs() < 0.0001);
    }

    #[test]
    #[serial]
    fn two_tier_exactly_at_boundary() {
        let catalog = make_two_tier_catalog();
        let usage = TokenUsage {
            input_tokens: 200_000,
            output_tokens: 0,
            ..zero_usage()
        };
        let cost = estimate_cost_with_catalog(
            &catalog,
            "claude-sonnet-4-20250514",
            usage,
            None,
            None,
            CostMode::Calculate,
        )
        .unwrap();
        let expected = 200_000.0 * 3.0 / 1_000_000.0;
        assert!((cost - expected).abs() < 0.0001);
    }

    #[test]
    #[serial]
    fn two_tier_one_above_boundary() {
        let catalog = make_two_tier_catalog();
        let usage = TokenUsage {
            input_tokens: 200_001,
            output_tokens: 0,
            ..zero_usage()
        };
        let cost = estimate_cost_with_catalog(
            &catalog,
            "claude-sonnet-4-20250514",
            usage,
            None,
            None,
            CostMode::Calculate,
        )
        .unwrap();
        let expected = 200_000.0 * 3.0 / 1_000_000.0 + 1.0 * 6.0 / 1_000_000.0;
        assert!((cost - expected).abs() < 0.0001);
    }

    #[test]
    #[serial]
    fn two_tier_far_above_last_tier() {
        let catalog = make_two_tier_catalog();
        let usage = TokenUsage {
            input_tokens: 10_000_000,
            output_tokens: 0,
            ..zero_usage()
        };
        let cost = estimate_cost_with_catalog(
            &catalog,
            "claude-sonnet-4-20250514",
            usage,
            None,
            None,
            CostMode::Calculate,
        )
        .unwrap();
        let expected = 200_000.0 * 3.0 / 1_000_000.0 + 9_800_000.0 * 6.0 / 1_000_000.0;
        assert!((cost - expected).abs() < 0.0001);
    }

    #[test]
    #[serial]
    fn two_tier_512k_threshold() {
        let catalog = PriceCatalog {
            schema_version: "3".to_string(),
            version: "test".to_string(),
            updated: "test".to_string(),
            aliases: HashMap::new(),
            prices: vec![ModelPrice {
                provider: "minimax".to_string(),
                model: "minimax-m1".to_string(),
                currency: "USD".to_string(),
                effective_date: "2099-01-01".to_string(),
                fast_multiplier: None,
                tier_mode: TierMode::Marginal,
                tiers: vec![
                    PriceTier {
                        from_tokens: 0,
                        input_per_million: 0.3,
                        output_per_million: 1.2,
                        cached_input_per_million: None,
                        cache_write_per_million: None,
                        cache_storage_per_million_hour: None,
                        reasoning_per_million: None,
                    },
                    PriceTier {
                        from_tokens: 512000,
                        input_per_million: 0.6,
                        output_per_million: 2.4,
                        cached_input_per_million: None,
                        cache_write_per_million: None,
                        cache_storage_per_million_hour: None,
                        reasoning_per_million: None,
                    },
                ],
            }],
        };
        let usage = TokenUsage {
            input_tokens: 1_000_000,
            output_tokens: 0,
            ..zero_usage()
        };
        let cost = estimate_cost_with_catalog(
            &catalog,
            "minimax-m1",
            usage,
            None,
            None,
            CostMode::Calculate,
        )
        .unwrap();
        let expected = 512_000.0 * 0.3 / 1_000_000.0 + 488_000.0 * 0.6 / 1_000_000.0;
        assert!((cost - expected).abs() < 0.0001);
    }

    // -- three-tier pricing test --

    #[test]
    #[serial]
    fn three_tier_correct_cost() {
        let catalog = PriceCatalog {
            schema_version: "3".to_string(),
            version: "test".to_string(),
            updated: "test".to_string(),
            aliases: HashMap::new(),
            prices: vec![ModelPrice {
                provider: "test".to_string(),
                model: "three-tier-model".to_string(),
                currency: "USD".to_string(),
                effective_date: "2099-01-01".to_string(),
                fast_multiplier: None,
                tier_mode: TierMode::Marginal,
                tiers: vec![
                    PriceTier {
                        from_tokens: 0,
                        input_per_million: 1.0,
                        output_per_million: 2.0,
                        cached_input_per_million: None,
                        cache_write_per_million: None,
                        cache_storage_per_million_hour: None,
                        reasoning_per_million: None,
                    },
                    PriceTier {
                        from_tokens: 200000,
                        input_per_million: 2.0,
                        output_per_million: 4.0,
                        cached_input_per_million: None,
                        cache_write_per_million: None,
                        cache_storage_per_million_hour: None,
                        reasoning_per_million: None,
                    },
                    PriceTier {
                        from_tokens: 1000000,
                        input_per_million: 3.0,
                        output_per_million: 6.0,
                        cached_input_per_million: None,
                        cache_write_per_million: None,
                        cache_storage_per_million_hour: None,
                        reasoning_per_million: None,
                    },
                ],
            }],
        };
        let usage = TokenUsage {
            input_tokens: 1_500_000,
            output_tokens: 0,
            ..zero_usage()
        };
        let cost = estimate_cost_with_catalog(
            &catalog,
            "three-tier-model",
            usage,
            None,
            None,
            CostMode::Calculate,
        )
        .unwrap();
        let expected = 200_000.0 * 1.0 / 1_000_000.0
            + 800_000.0 * 2.0 / 1_000_000.0
            + 500_000.0 * 3.0 / 1_000_000.0;
        assert!((cost - expected).abs() < 0.0001);
    }

    // -- cache read/write pricing tests --

    #[test]
    #[serial]
    fn cache_read_tokens_priced_at_cached_input_rate() {
        let catalog = make_two_tier_catalog();
        let usage = TokenUsage {
            input_tokens: 200_000,
            output_tokens: 0,
            cached_input_tokens: 100_000,
            cache_creation_tokens: 0,
            reasoning_tokens: 0,
        };
        let cost = estimate_cost_with_catalog(
            &catalog,
            "claude-sonnet-4-20250514",
            usage,
            None,
            None,
            CostMode::Calculate,
        )
        .unwrap();
        let expected = 100_000.0 * 3.0 / 1_000_000.0 + 100_000.0 * 0.3 / 1_000_000.0;
        assert!((cost - expected).abs() < 0.0001);
    }

    #[test]
    #[serial]
    fn cache_write_tokens_priced_at_cache_write_rate() {
        let catalog = make_two_tier_catalog();
        let usage = TokenUsage {
            input_tokens: 200_000,
            output_tokens: 0,
            cached_input_tokens: 0,
            cache_creation_tokens: 50_000,
            reasoning_tokens: 0,
        };
        let cost = estimate_cost_with_catalog(
            &catalog,
            "claude-sonnet-4-20250514",
            usage,
            None,
            None,
            CostMode::Calculate,
        )
        .unwrap();
        let expected = 150_000.0 * 3.0 / 1_000_000.0 + 50_000.0 * 3.75 / 1_000_000.0;
        assert!((cost - expected).abs() < 0.0001);
    }

    #[test]
    #[serial]
    fn cache_tokens_cross_tier_boundary() {
        let catalog = make_two_tier_catalog();
        let usage = TokenUsage {
            input_tokens: 500_000,
            output_tokens: 0,
            cached_input_tokens: 200_000,
            cache_creation_tokens: 50_000,
            reasoning_tokens: 0,
        };
        let cost = estimate_cost_with_catalog(
            &catalog,
            "claude-sonnet-4-20250514",
            usage,
            None,
            None,
            CostMode::Calculate,
        )
        .unwrap();
        // non_cached = 500_000 - 200_000 - 50_000 = 250_000
        // non_cached: 200k @ 3.0 + 50k @ 6.0
        // cached = 200k, exactly at 200k boundary -> all tier 0 @ 0.3
        // creation = 50k @ 3.75
        let non_cached_cost = 200_000.0 * 3.0 / 1_000_000.0 + 50_000.0 * 6.0 / 1_000_000.0;
        let cached_cost = 200_000.0 * 0.3 / 1_000_000.0;
        let creation_cost = 50_000.0 * 3.75 / 1_000_000.0;
        let expected = non_cached_cost + cached_cost + creation_cost;
        assert!((cost - expected).abs() < 0.0001);
    }

    // -- clamping tests --

    #[test]
    #[serial]
    fn cached_tokens_clamped_to_input_tokens() {
        let catalog = make_single_tier_catalog("gpt-5", 1.25, 10.0);
        let usage = TokenUsage {
            input_tokens: 100,
            output_tokens: 0,
            cached_input_tokens: 200, // exceeds input
            cache_creation_tokens: 0,
            reasoning_tokens: 0,
        };
        let cost = estimate_cost_with_catalog(&catalog, "gpt-5", usage, None, None, CostMode::Auto)
            .unwrap();
        // cached clamped to 100, non_cached = 0, catalog has no cached_input rate
        let expected = 0.0;
        assert!((cost - expected).abs() < 0.0001);
    }

    #[test]
    #[serial]
    fn cache_creation_clamped_when_exceeds_input() {
        let catalog = make_single_tier_catalog("gpt-5", 1.25, 10.0);
        let usage = TokenUsage {
            input_tokens: 100,
            output_tokens: 0,
            cached_input_tokens: 0,
            cache_creation_tokens: 200, // exceeds input
            reasoning_tokens: 0,
        };
        let cost = estimate_cost_with_catalog(&catalog, "gpt-5", usage, None, None, CostMode::Auto)
            .unwrap();
        // creation clamped to 100, non_cached = 0, catalog has no cache_write rate
        let expected = 0.0;
        assert!((cost - expected).abs() < 0.0001);
    }

    #[test]
    #[serial]
    fn cache_sum_clamped_to_input() {
        let catalog = make_single_tier_catalog("gpt-5", 1.25, 10.0);
        let usage = TokenUsage {
            input_tokens: 200,
            output_tokens: 0,
            cached_input_tokens: 150,
            cache_creation_tokens: 150, // 150 + 150 = 300 > 200 → creation clamped to 50
            reasoning_tokens: 0,
        };
        let cost = estimate_cost_with_catalog(&catalog, "gpt-5", usage, None, None, CostMode::Auto)
            .unwrap();
        // cached = 150, creation clamped to 50, non_cached = 200 - 150 - 50 = 0
        // catalog has no cached_input or cache_write rate, so only non_cached matters
        let expected = 0.0;
        assert!((cost - expected).abs() < 0.0001);
    }

    // -- null price handling --

    #[test]
    #[serial]
    fn null_cache_write_with_zero_tokens_no_error() {
        let catalog = make_single_tier_catalog("gpt-5", 1.25, 10.0);
        let usage = TokenUsage {
            input_tokens: 100_000,
            output_tokens: 50_000,
            ..zero_usage()
        };
        let cost =
            estimate_cost_with_catalog(&catalog, "gpt-5", usage, None, None, CostMode::Calculate)
                .unwrap();
        let expected = 100_000.0 * 1.25 / 1_000_000.0 + 50_000.0 * 10.0 / 1_000_000.0;
        assert!((cost - expected).abs() < 0.0001);
    }

    // -- fast_multiplier tests --

    #[test]
    #[serial]
    fn fast_multiplier_applied() {
        let catalog = make_two_tier_catalog();
        let usage = TokenUsage {
            input_tokens: 100_000,
            output_tokens: 50_000,
            ..zero_usage()
        };
        let normal = estimate_cost_with_catalog(
            &catalog,
            "claude-sonnet-4-20250514",
            usage,
            None,
            None,
            CostMode::Calculate,
        )
        .unwrap();
        let fast = estimate_cost_with_catalog(
            &catalog,
            "claude-sonnet-4-20250514",
            usage,
            None,
            Some("fast"),
            CostMode::Calculate,
        )
        .unwrap();
        assert!((fast - normal * 6.0).abs() < 0.0001);
    }

    #[test]
    #[serial]
    fn fast_mode_no_multiplier_when_null() {
        let catalog = make_single_tier_catalog("gpt-5", 1.25, 10.0);
        let usage = TokenUsage {
            input_tokens: 100_000,
            output_tokens: 50_000,
            ..zero_usage()
        };
        let normal =
            estimate_cost_with_catalog(&catalog, "gpt-5", usage, None, None, CostMode::Calculate)
                .unwrap();
        let fast = estimate_cost_with_catalog(
            &catalog,
            "gpt-5",
            usage,
            None,
            Some("fast"),
            CostMode::Calculate,
        )
        .unwrap();
        assert!((fast - normal).abs() < 0.0001);
    }

    // -- hot-reload lifecycle tests --

    #[test]
    #[serial]
    fn catalog_lifecycle_init_reload_and_error_cases() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("price-catalog.json");

        reset_catalog();
        init_catalog(Some(&path));
        let catalog = load_catalog();
        assert!(!catalog.prices.is_empty());
        assert_eq!(catalog.schema_version, "3");

        reset_catalog();
        let v1 = r#"{"schema_version":"3","version":"2099-v1","updated":"2099-01-01","aliases":{},"prices":[{"provider":"test","model":"test-model","currency":"USD","effective_date":"2099-01-01","fast_multiplier":null,"tiers":[{"from_tokens":0,"input_per_million":1.0,"output_per_million":2.0,"cached_input_per_million":null,"cache_write_per_million":null,"cache_storage_per_million_hour":null,"reasoning_per_million":null}]}]}"#;
        write_file_with_distinct_mtime(&path, v1);
        init_catalog(Some(&path));
        let catalog = load_catalog();
        assert_eq!(catalog.version, "2099-v1");

        let result = try_reload_catalog(&path);
        assert!(matches!(result, ReloadResult::Unchanged));

        let v2 = r#"{"schema_version":"3","version":"2099-v2","updated":"2099-06-01","aliases":{},"prices":[{"provider":"test","model":"test-model","currency":"USD","effective_date":"2099-06-01","fast_multiplier":null,"tiers":[{"from_tokens":0,"input_per_million":5.0,"output_per_million":10.0,"cached_input_per_million":null,"cache_write_per_million":null,"cache_storage_per_million_hour":null,"reasoning_per_million":null}]}]}"#;
        write_file_with_distinct_mtime(&path, v2);
        let result = try_reload_catalog(&path);
        match result {
            ReloadResult::Reloaded { version } => assert_eq!(version, "2099-v2"),
            other => panic!("expected Reloaded, got {:?}", other),
        }

        write_file_with_distinct_mtime(&path, "not valid json{{{");
        assert!(matches!(
            try_reload_catalog(&path),
            ReloadResult::ParseError { .. }
        ));
        assert_eq!(load_catalog().version, "2099-v2");

        let empty = r#"{"schema_version":"3","version":"2099-empty","updated":"2099-01-01","aliases":{},"prices":[]}"#;
        write_file_with_distinct_mtime(&path, empty);
        assert!(matches!(
            try_reload_catalog(&path),
            ReloadResult::Invalid { .. }
        ));
        assert_eq!(load_catalog().version, "2099-v2");

        let missing = tmp.path().join("does-not-exist.json");
        assert!(matches!(
            try_reload_catalog(&missing),
            ReloadResult::Missing
        ));

        let dir_path = tmp.path().join("a-directory");
        std::fs::create_dir_all(&dir_path).unwrap();
        assert!(matches!(
            try_reload_catalog(&dir_path),
            ReloadResult::IoError { .. }
        ));

        reset_catalog();
        init_catalog(None::<&std::path::Path>);
    }

    // -- schema validation tests --

    #[test]
    fn validate_rejects_tiers_first_not_zero() {
        let json = r#"{"schema_version":"3","version":"v1","updated":"v1","aliases":{},"prices":[{"provider":"test","model":"m","currency":"USD","effective_date":"2099-01-01","fast_multiplier":null,"tiers":[{"from_tokens":100,"input_per_million":1.0,"output_per_million":2.0,"cached_input_per_million":null,"cache_write_per_million":null,"cache_storage_per_million_hour":null,"reasoning_per_million":null}]}]}"#;
        let result = parse_catalog_json(json);
        assert!(matches!(result, Err(ReloadResult::Invalid { .. })));
    }

    #[test]
    fn validate_rejects_tiers_not_strictly_increasing() {
        let json = r#"{"schema_version":"3","version":"v1","updated":"v1","aliases":{},"prices":[{"provider":"test","model":"m","currency":"USD","effective_date":"2099-01-01","fast_multiplier":null,"tiers":[{"from_tokens":0,"input_per_million":1.0,"output_per_million":2.0,"cached_input_per_million":null,"cache_write_per_million":null,"cache_storage_per_million_hour":null,"reasoning_per_million":null},{"from_tokens":0,"input_per_million":2.0,"output_per_million":4.0,"cached_input_per_million":null,"cache_write_per_million":null,"cache_storage_per_million_hour":null,"reasoning_per_million":null}]}]}"#;
        let result = parse_catalog_json(json);
        assert!(matches!(result, Err(ReloadResult::Invalid { .. })));
    }

    #[test]
    fn validate_rejects_empty_tiers() {
        let json = r#"{"schema_version":"3","version":"v1","updated":"v1","aliases":{},"prices":[{"provider":"test","model":"m","currency":"USD","effective_date":"2099-01-01","fast_multiplier":null,"tiers":[]}]}"#;
        let result = parse_catalog_json(json);
        assert!(matches!(result, Err(ReloadResult::Invalid { .. })));
    }

    #[test]
    fn validate_rejects_duplicate_model() {
        let json = r#"{"schema_version":"3","version":"v1","updated":"v1","aliases":{},"prices":[{"provider":"test","model":"m","currency":"USD","effective_date":"2099-01-01","fast_multiplier":null,"tiers":[{"from_tokens":0,"input_per_million":1.0,"output_per_million":2.0,"cached_input_per_million":null,"cache_write_per_million":null,"cache_storage_per_million_hour":null,"reasoning_per_million":null}]},{"provider":"test","model":"m","currency":"USD","effective_date":"2099-01-01","fast_multiplier":null,"tiers":[{"from_tokens":0,"input_per_million":1.0,"output_per_million":2.0,"cached_input_per_million":null,"cache_write_per_million":null,"cache_storage_per_million_hour":null,"reasoning_per_million":null}]}]}"#;
        let result = parse_catalog_json(json);
        assert!(matches!(result, Err(ReloadResult::Invalid { .. })));
    }

    #[test]
    fn validate_rejects_alias_target_not_found() {
        let json = r#"{"schema_version":"3","version":"v1","updated":"v1","aliases":{"foo":"nonexistent"},"prices":[{"provider":"test","model":"m","currency":"USD","effective_date":"2099-01-01","fast_multiplier":null,"tiers":[{"from_tokens":0,"input_per_million":1.0,"output_per_million":2.0,"cached_input_per_million":null,"cache_write_per_million":null,"cache_storage_per_million_hour":null,"reasoning_per_million":null}]}]}"#;
        let result = parse_catalog_json(json);
        assert!(matches!(result, Err(ReloadResult::Invalid { .. })));
    }

    #[test]
    fn validate_rejects_fast_multiplier_zero() {
        let json = r#"{"schema_version":"3","version":"v1","updated":"v1","aliases":{},"prices":[{"provider":"test","model":"m","currency":"USD","effective_date":"2099-01-01","fast_multiplier":0.0,"tiers":[{"from_tokens":0,"input_per_million":1.0,"output_per_million":2.0,"cached_input_per_million":null,"cache_write_per_million":null,"cache_storage_per_million_hour":null,"reasoning_per_million":null}]}]}"#;
        let result = parse_catalog_json(json);
        assert!(matches!(result, Err(ReloadResult::Invalid { .. })));
    }

    #[test]
    fn validate_rejects_null_for_required_price_field() {
        let json = r#"{"schema_version":"3","version":"v1","updated":"v1","aliases":{},"prices":[{"provider":"test","model":"m","currency":"USD","effective_date":"2099-01-01","fast_multiplier":null,"tiers":[{"from_tokens":0,"input_per_million":null,"output_per_million":2.0,"cached_input_per_million":null,"cache_write_per_million":null,"cache_storage_per_million_hour":null,"reasoning_per_million":null}]}]}"#;
        let result: Result<PriceCatalog, _> = serde_json::from_str(json);
        assert!(result.is_err());
    }

    #[test]
    fn validate_rejects_fast_multiplier_negative() {
        let json = r#"{"schema_version":"3","version":"v1","updated":"v1","aliases":{},"prices":[{"provider":"test","model":"m","currency":"USD","effective_date":"2099-01-01","fast_multiplier":-1.0,"tiers":[{"from_tokens":0,"input_per_million":1.0,"output_per_million":2.0,"cached_input_per_million":null,"cache_write_per_million":null,"cache_storage_per_million_hour":null,"reasoning_per_million":null}]}]}"#;
        let result = parse_catalog_json(json);
        assert!(matches!(result, Err(ReloadResult::Invalid { .. })));
    }

    #[test]
    fn validate_rejects_fast_multiplier_infinity() {
        let json = r#"{"schema_version":"3","version":"v1","updated":"v1","aliases":{},"prices":[{"provider":"test","model":"m","currency":"USD","effective_date":"2099-01-01","fast_multiplier":1e9999,"tiers":[{"from_tokens":0,"input_per_million":1.0,"output_per_million":2.0,"cached_input_per_million":null,"cache_write_per_million":null,"cache_storage_per_million_hour":null,"reasoning_per_million":null}]}]}"#;
        let result: Result<PriceCatalog, _> = serde_json::from_str(json);
        assert!(result.is_err());
    }

    #[test]
    fn validate_rejects_old_flat_fields_at_top_level() {
        let json = r#"{"schema_version":"3","version":"v1","updated":"v1","aliases":{},"prices":[{"provider":"test","model":"m","currency":"USD","effective_date":"2099-01-01","fast_multiplier":null,"input_per_million":1.0,"tiers":[{"from_tokens":0,"input_per_million":1.0,"output_per_million":2.0,"cached_input_per_million":null,"cache_write_per_million":null,"cache_storage_per_million_hour":null,"reasoning_per_million":null}]}]}"#;
        let result: Result<PriceCatalog, _> = serde_json::from_str(json);
        assert!(result.is_err());
    }

    #[test]
    fn validate_rejects_unknown_fields_in_tier() {
        let json = r#"{"schema_version":"3","version":"v1","updated":"v1","aliases":{},"prices":[{"provider":"test","model":"m","currency":"USD","effective_date":"2099-01-01","fast_multiplier":null,"tiers":[{"from_tokens":0,"input_per_million":1.0,"output_per_million":2.0,"cached_input_per_million":null,"cache_write_per_million":null,"cache_storage_per_million_hour":null,"reasoning_per_million":null,"input_per_million_above_200k":5.0}]}]}"#;
        let result: Result<PriceCatalog, _> = serde_json::from_str(json);
        assert!(result.is_err());
    }

    #[test]
    fn valid_catalog_loads_successfully() {
        let json = r#"{"schema_version":"3","version":"v1","updated":"v1","aliases":{},"prices":[{"provider":"test","model":"m","currency":"USD","effective_date":"2099-01-01","fast_multiplier":null,"tiers":[{"from_tokens":0,"input_per_million":1.0,"output_per_million":2.0,"cached_input_per_million":null,"cache_write_per_million":null,"cache_storage_per_million_hour":null,"reasoning_per_million":null}]}]}"#;
        let catalog = parse_catalog_json(json).unwrap();
        assert_eq!(catalog.prices.len(), 1);
        assert_eq!(catalog.prices[0].model, "m");
    }

    // -- whole-request tier mode tests --

    fn make_whole_request_catalog() -> PriceCatalog {
        PriceCatalog {
            schema_version: "3".to_string(),
            version: "test".to_string(),
            updated: "test".to_string(),
            aliases: HashMap::new(),
            prices: vec![ModelPrice {
                provider: "google".to_string(),
                model: "test-whole-request".to_string(),
                currency: "USD".to_string(),
                effective_date: "2099-01-01".to_string(),
                fast_multiplier: None,
                tier_mode: TierMode::WholeRequest,
                tiers: vec![
                    PriceTier {
                        from_tokens: 0,
                        input_per_million: 1.25,
                        output_per_million: 10.0,
                        cached_input_per_million: Some(0.125),
                        cache_write_per_million: None,
                        cache_storage_per_million_hour: None,
                        reasoning_per_million: None,
                    },
                    PriceTier {
                        from_tokens: 200001,
                        input_per_million: 2.50,
                        output_per_million: 15.0,
                        cached_input_per_million: Some(0.25),
                        cache_write_per_million: None,
                        cache_storage_per_million_hour: None,
                        reasoning_per_million: None,
                    },
                ],
            }],
        }
    }

    #[test]
    #[serial]
    fn whole_request_below_200k_uses_tier_0() {
        let catalog = make_whole_request_catalog();
        let usage = TokenUsage {
            input_tokens: 100_000,
            output_tokens: 50_000,
            ..zero_usage()
        };
        let cost = estimate_cost_with_catalog(
            &catalog,
            "test-whole-request",
            usage,
            None,
            None,
            CostMode::Calculate,
        )
        .unwrap();
        let expected = 100_000.0 * 1.25 / 1_000_000.0 + 50_000.0 * 10.0 / 1_000_000.0;
        assert!((cost - expected).abs() < 0.0001);
    }

    #[test]
    #[serial]
    fn whole_request_above_200k_uses_tier_1() {
        let catalog = make_whole_request_catalog();
        let usage = TokenUsage {
            input_tokens: 300_000,
            output_tokens: 50_000,
            ..zero_usage()
        };
        let cost = estimate_cost_with_catalog(
            &catalog,
            "test-whole-request",
            usage,
            None,
            None,
            CostMode::Calculate,
        )
        .unwrap();
        // Key difference from marginal: all tokens at tier 1 rates, NOT segmented
        let expected = 300_000.0 * 2.50 / 1_000_000.0 + 50_000.0 * 15.0 / 1_000_000.0;
        assert!((cost - expected).abs() < 0.0001);
    }

    #[test]
    #[serial]
    fn whole_request_exactly_200k_uses_tier_0() {
        let catalog = make_whole_request_catalog();
        let usage = TokenUsage {
            input_tokens: 200_000,
            output_tokens: 0,
            ..zero_usage()
        };
        let cost = estimate_cost_with_catalog(
            &catalog,
            "test-whole-request",
            usage,
            None,
            None,
            CostMode::Calculate,
        )
        .unwrap();
        let expected = 200_000.0 * 1.25 / 1_000_000.0;
        assert!((cost - expected).abs() < 0.0001);
    }

    #[test]
    #[serial]
    fn whole_request_200001_uses_tier_1() {
        let catalog = make_whole_request_catalog();
        let usage = TokenUsage {
            input_tokens: 200_001,
            output_tokens: 0,
            ..zero_usage()
        };
        let cost = estimate_cost_with_catalog(
            &catalog,
            "test-whole-request",
            usage,
            None,
            None,
            CostMode::Calculate,
        )
        .unwrap();
        let expected = 200_001.0 * 2.50 / 1_000_000.0;
        assert!((cost - expected).abs() < 0.0001);
    }

    #[test]
    #[serial]
    fn whole_request_with_cached_tokens() {
        let catalog = make_whole_request_catalog();
        let usage = TokenUsage {
            input_tokens: 300_000,
            output_tokens: 0,
            cached_input_tokens: 200_000,
            cache_creation_tokens: 0,
            reasoning_tokens: 0,
        };
        let cost = estimate_cost_with_catalog(
            &catalog,
            "test-whole-request",
            usage,
            None,
            None,
            CostMode::Calculate,
        )
        .unwrap();
        // Tier 1 selected (300k > 200k). non_cached = 100k, cached = 200k.
        let expected = 100_000.0 * 2.50 / 1_000_000.0 + 200_000.0 * 0.25 / 1_000_000.0;
        assert!((cost - expected).abs() < 0.0001);
    }

    #[test]
    #[serial]
    fn whole_request_with_cache_creation() {
        let catalog = make_whole_request_catalog();
        let usage = TokenUsage {
            input_tokens: 300_000,
            output_tokens: 0,
            cached_input_tokens: 0,
            cache_creation_tokens: 50_000,
            reasoning_tokens: 0,
        };
        let cost = estimate_cost_with_catalog(
            &catalog,
            "test-whole-request",
            usage,
            None,
            None,
            CostMode::Calculate,
        )
        .unwrap();
        // Tier 1 selected. non_cached = 250k, creation = 50k but cache_write is null → 0.
        let expected = 250_000.0 * 2.50 / 1_000_000.0;
        assert!((cost - expected).abs() < 0.0001);
    }

    #[test]
    #[serial]
    fn whole_request_single_tier() {
        let catalog = PriceCatalog {
            schema_version: "3".to_string(),
            version: "test".to_string(),
            updated: "test".to_string(),
            aliases: HashMap::new(),
            prices: vec![ModelPrice {
                provider: "google".to_string(),
                model: "test-whole-single".to_string(),
                currency: "USD".to_string(),
                effective_date: "2099-01-01".to_string(),
                fast_multiplier: None,
                tier_mode: TierMode::WholeRequest,
                tiers: vec![PriceTier {
                    from_tokens: 0,
                    input_per_million: 0.50,
                    output_per_million: 3.0,
                    cached_input_per_million: Some(0.05),
                    cache_write_per_million: None,
                    cache_storage_per_million_hour: None,
                    reasoning_per_million: None,
                }],
            }],
        };
        let usage = TokenUsage {
            input_tokens: 500_000,
            output_tokens: 100_000,
            ..zero_usage()
        };
        let cost = estimate_cost_with_catalog(
            &catalog,
            "test-whole-single",
            usage,
            None,
            None,
            CostMode::Calculate,
        )
        .unwrap();
        let expected = 500_000.0 * 0.50 / 1_000_000.0 + 100_000.0 * 3.0 / 1_000_000.0;
        assert!((cost - expected).abs() < 0.0001);
    }

    #[test]
    fn serde_default_tier_mode_is_marginal() {
        let json = r#"{"schema_version":"3","version":"v1","updated":"v1","aliases":{},"prices":[{"provider":"test","model":"m","currency":"USD","effective_date":"2099-01-01","fast_multiplier":null,"tiers":[{"from_tokens":0,"input_per_million":1.0,"output_per_million":2.0,"cached_input_per_million":null,"cache_write_per_million":null,"cache_storage_per_million_hour":null,"reasoning_per_million":null}]}]}"#;
        let catalog = parse_catalog_json(json).unwrap();
        assert_eq!(catalog.prices[0].tier_mode, TierMode::Marginal);
    }

    #[test]
    fn serde_rejects_unknown_tier_mode() {
        let json = r#"{"schema_version":"3","version":"v1","updated":"v1","aliases":{},"prices":[{"provider":"test","model":"m","currency":"USD","effective_date":"2099-01-01","fast_multiplier":null,"tier_mode":"invalid","tiers":[{"from_tokens":0,"input_per_million":1.0,"output_per_million":2.0,"cached_input_per_million":null,"cache_write_per_million":null,"cache_storage_per_million_hour":null,"reasoning_per_million":null}]}]}"#;
        let result: Result<PriceCatalog, _> = serde_json::from_str(json);
        assert!(result.is_err());
    }

    #[test]
    #[serial]
    fn whole_request_with_fast_multiplier() {
        let catalog = PriceCatalog {
            schema_version: "3".to_string(),
            version: "test".to_string(),
            updated: "test".to_string(),
            aliases: HashMap::new(),
            prices: vec![ModelPrice {
                provider: "google".to_string(),
                model: "test-whole-fast".to_string(),
                currency: "USD".to_string(),
                effective_date: "2099-01-01".to_string(),
                fast_multiplier: Some(2.0),
                tier_mode: TierMode::WholeRequest,
                tiers: vec![
                    PriceTier {
                        from_tokens: 0,
                        input_per_million: 1.0,
                        output_per_million: 5.0,
                        cached_input_per_million: None,
                        cache_write_per_million: None,
                        cache_storage_per_million_hour: None,
                        reasoning_per_million: None,
                    },
                    PriceTier {
                        from_tokens: 200001,
                        input_per_million: 2.0,
                        output_per_million: 10.0,
                        cached_input_per_million: None,
                        cache_write_per_million: None,
                        cache_storage_per_million_hour: None,
                        reasoning_per_million: None,
                    },
                ],
            }],
        };
        let usage = TokenUsage {
            input_tokens: 300_000,
            output_tokens: 100_000,
            ..zero_usage()
        };
        let normal = estimate_cost_with_catalog(
            &catalog,
            "test-whole-fast",
            usage,
            None,
            None,
            CostMode::Calculate,
        )
        .unwrap();
        let fast = estimate_cost_with_catalog(
            &catalog,
            "test-whole-fast",
            usage,
            None,
            Some("fast"),
            CostMode::Calculate,
        )
        .unwrap();
        let expected_normal = 300_000.0 * 2.0 / 1_000_000.0 + 100_000.0 * 10.0 / 1_000_000.0;
        assert!((normal - expected_normal).abs() < 0.0001);
        assert!((fast - normal * 2.0).abs() < 0.0001);
    }

    #[test]
    #[serial]
    fn whole_request_with_reasoning_tokens() {
        let catalog = PriceCatalog {
            schema_version: "3".to_string(),
            version: "test".to_string(),
            updated: "test".to_string(),
            aliases: HashMap::new(),
            prices: vec![ModelPrice {
                provider: "google".to_string(),
                model: "test-whole-reasoning".to_string(),
                currency: "USD".to_string(),
                effective_date: "2099-01-01".to_string(),
                fast_multiplier: None,
                tier_mode: TierMode::WholeRequest,
                tiers: vec![
                    PriceTier {
                        from_tokens: 0,
                        input_per_million: 1.0,
                        output_per_million: 5.0,
                        cached_input_per_million: None,
                        cache_write_per_million: None,
                        cache_storage_per_million_hour: None,
                        reasoning_per_million: Some(3.0),
                    },
                    PriceTier {
                        from_tokens: 200001,
                        input_per_million: 2.0,
                        output_per_million: 10.0,
                        cached_input_per_million: None,
                        cache_write_per_million: None,
                        cache_storage_per_million_hour: None,
                        reasoning_per_million: Some(6.0),
                    },
                ],
            }],
        };
        let usage = TokenUsage {
            input_tokens: 300_000,
            output_tokens: 0,
            cached_input_tokens: 0,
            cache_creation_tokens: 0,
            reasoning_tokens: 100_000,
        };
        let cost = estimate_cost_with_catalog(
            &catalog,
            "test-whole-reasoning",
            usage,
            None,
            None,
            CostMode::Calculate,
        )
        .unwrap();
        // Tier 1 selected (300k > 200k). All at tier 1 rates.
        let expected = 300_000.0 * 2.0 / 1_000_000.0 + 100_000.0 * 6.0 / 1_000_000.0;
        assert!((cost - expected).abs() < 0.0001);
    }

    #[test]
    #[serial]
    fn whole_request_mixed_input_all_categories() {
        let catalog = make_whole_request_catalog();
        let usage = TokenUsage {
            input_tokens: 500_000,
            output_tokens: 50_000,
            cached_input_tokens: 100_000,
            cache_creation_tokens: 50_000,
            reasoning_tokens: 0,
        };
        let cost = estimate_cost_with_catalog(
            &catalog,
            "test-whole-request",
            usage,
            None,
            None,
            CostMode::Calculate,
        )
        .unwrap();
        // Tier 1 selected (500k > 200k). non_cached = 350k, cached = 100k, creation = 50k (null rate).
        let expected = 350_000.0 * 2.50 / 1_000_000.0
            + 100_000.0 * 0.25 / 1_000_000.0
            + 50_000.0 * 15.0 / 1_000_000.0;
        assert!((cost - expected).abs() < 0.0001);
    }

    #[test]
    #[serial]
    fn whole_request_three_tier_selects_middle() {
        let catalog = PriceCatalog {
            schema_version: "3".to_string(),
            version: "test".to_string(),
            updated: "test".to_string(),
            aliases: HashMap::new(),
            prices: vec![ModelPrice {
                provider: "test".to_string(),
                model: "three-tier-whole".to_string(),
                currency: "USD".to_string(),
                effective_date: "2099-01-01".to_string(),
                fast_multiplier: None,
                tier_mode: TierMode::WholeRequest,
                tiers: vec![
                    PriceTier {
                        from_tokens: 0,
                        input_per_million: 1.0,
                        output_per_million: 2.0,
                        cached_input_per_million: None,
                        cache_write_per_million: None,
                        cache_storage_per_million_hour: None,
                        reasoning_per_million: None,
                    },
                    PriceTier {
                        from_tokens: 128000,
                        input_per_million: 2.0,
                        output_per_million: 4.0,
                        cached_input_per_million: None,
                        cache_write_per_million: None,
                        cache_storage_per_million_hour: None,
                        reasoning_per_million: None,
                    },
                    PriceTier {
                        from_tokens: 200001,
                        input_per_million: 3.0,
                        output_per_million: 6.0,
                        cached_input_per_million: None,
                        cache_write_per_million: None,
                        cache_storage_per_million_hour: None,
                        reasoning_per_million: None,
                    },
                ],
            }],
        };
        // Middle tier (128k-200k)
        let usage_mid = TokenUsage {
            input_tokens: 150_000,
            output_tokens: 10_000,
            ..zero_usage()
        };
        let cost_mid = estimate_cost_with_catalog(
            &catalog,
            "three-tier-whole",
            usage_mid,
            None,
            None,
            CostMode::Calculate,
        )
        .unwrap();
        let expected_mid = 150_000.0 * 2.0 / 1_000_000.0 + 10_000.0 * 4.0 / 1_000_000.0;
        assert!((cost_mid - expected_mid).abs() < 0.0001);

        // Top tier (>200k)
        let usage_top = TokenUsage {
            input_tokens: 500_000,
            output_tokens: 10_000,
            ..zero_usage()
        };
        let cost_top = estimate_cost_with_catalog(
            &catalog,
            "three-tier-whole",
            usage_top,
            None,
            None,
            CostMode::Calculate,
        )
        .unwrap();
        let expected_top = 500_000.0 * 3.0 / 1_000_000.0 + 10_000.0 * 6.0 / 1_000_000.0;
        assert!((cost_top - expected_top).abs() < 0.0001);

        // Bottom tier (<128k)
        let usage_bot = TokenUsage {
            input_tokens: 50_000,
            output_tokens: 10_000,
            ..zero_usage()
        };
        let cost_bot = estimate_cost_with_catalog(
            &catalog,
            "three-tier-whole",
            usage_bot,
            None,
            None,
            CostMode::Calculate,
        )
        .unwrap();
        let expected_bot = 50_000.0 * 1.0 / 1_000_000.0 + 10_000.0 * 2.0 / 1_000_000.0;
        assert!((cost_bot - expected_bot).abs() < 0.0001);
    }

    #[test]
    #[serial]
    fn whole_request_zero_input_nonzero_output() {
        let catalog = make_whole_request_catalog();
        let usage = TokenUsage {
            input_tokens: 0,
            output_tokens: 100_000,
            ..zero_usage()
        };
        let cost = estimate_cost_with_catalog(
            &catalog,
            "test-whole-request",
            usage,
            None,
            None,
            CostMode::Calculate,
        )
        .unwrap();
        // total_input=0 selects tier 0 (from_tokens=0). Output priced at tier 0 rate.
        let expected = 100_000.0 * 10.0 / 1_000_000.0;
        assert!((cost - expected).abs() < 0.0001);
    }

    #[test]
    #[serial]
    fn whole_request_warn_only_for_selected_tier_null() {
        // Tier 0 has cached_input_per_million: Some, tier 1 has None.
        // Below threshold → no warn. Above threshold → warn should fire.
        let catalog = PriceCatalog {
            schema_version: "3".to_string(),
            version: "test".to_string(),
            updated: "test".to_string(),
            aliases: HashMap::new(),
            prices: vec![ModelPrice {
                provider: "test".to_string(),
                model: "asymmetric-cache".to_string(),
                currency: "USD".to_string(),
                effective_date: "2099-01-01".to_string(),
                fast_multiplier: None,
                tier_mode: TierMode::WholeRequest,
                tiers: vec![
                    PriceTier {
                        from_tokens: 0,
                        input_per_million: 1.0,
                        output_per_million: 5.0,
                        cached_input_per_million: Some(0.1),
                        cache_write_per_million: None,
                        cache_storage_per_million_hour: None,
                        reasoning_per_million: None,
                    },
                    PriceTier {
                        from_tokens: 200001,
                        input_per_million: 2.0,
                        output_per_million: 10.0,
                        cached_input_per_million: None,
                        cache_write_per_million: None,
                        cache_storage_per_million_hour: None,
                        reasoning_per_million: None,
                    },
                ],
            }],
        };

        // Below threshold: tier 0 selected, has cached rate → no warn expected, cost includes cached
        let usage_below = TokenUsage {
            input_tokens: 100_000,
            output_tokens: 0,
            cached_input_tokens: 50_000,
            cache_creation_tokens: 0,
            reasoning_tokens: 0,
        };
        let cost_below = estimate_cost_with_catalog(
            &catalog,
            "asymmetric-cache",
            usage_below,
            None,
            None,
            CostMode::Calculate,
        )
        .unwrap();
        let expected_below = 50_000.0 * 1.0 / 1_000_000.0 + 50_000.0 * 0.1 / 1_000_000.0;
        assert!((cost_below - expected_below).abs() < 0.0001);

        // Above threshold: tier 1 selected, cached rate is None → cost=0 for cached, warn fires
        let usage_above = TokenUsage {
            input_tokens: 300_000,
            output_tokens: 0,
            cached_input_tokens: 100_000,
            cache_creation_tokens: 0,
            reasoning_tokens: 0,
        };
        let cost_above = estimate_cost_with_catalog(
            &catalog,
            "asymmetric-cache",
            usage_above,
            None,
            None,
            CostMode::Calculate,
        )
        .unwrap();
        // non_cached = 200k @ 2.0, cached = 100k @ null = 0
        let expected_above = 200_000.0 * 2.0 / 1_000_000.0;
        assert!((cost_above - expected_above).abs() < 0.0001);
    }

    // -- additional validation coverage tests --

    #[test]
    fn validate_rejects_unsupported_schema_version() {
        let json = r#"{"schema_version":"2","version":"v1","updated":"v1","aliases":{},"prices":[{"provider":"test","model":"m","currency":"USD","effective_date":"2099-01-01","fast_multiplier":null,"tiers":[{"from_tokens":0,"input_per_million":1.0,"output_per_million":2.0,"cached_input_per_million":null,"cache_write_per_million":null,"cache_storage_per_million_hour":null,"reasoning_per_million":null}]}]}"#;
        let result = parse_catalog_json(json);
        match result {
            Err(ReloadResult::Invalid { reason }) => {
                assert!(
                    reason.contains("unsupported schema_version"),
                    "got: {reason}"
                );
                assert!(reason.contains("expected '3'"), "got: {reason}");
            }
            other => panic!("expected Invalid, got {:?}", other),
        }
    }

    #[test]
    fn validate_rejects_empty_version() {
        let json = r#"{"schema_version":"3","version":"   ","updated":"v1","aliases":{},"prices":[{"provider":"test","model":"m","currency":"USD","effective_date":"2099-01-01","fast_multiplier":null,"tiers":[{"from_tokens":0,"input_per_million":1.0,"output_per_million":2.0,"cached_input_per_million":null,"cache_write_per_million":null,"cache_storage_per_million_hour":null,"reasoning_per_million":null}]}]}"#;
        let result = parse_catalog_json(json);
        match result {
            Err(ReloadResult::Invalid { reason }) => {
                assert!(reason.contains("catalog version is empty"), "got: {reason}");
            }
            other => panic!("expected Invalid, got {:?}", other),
        }
    }

    #[test]
    fn validate_rejects_empty_provider() {
        let json = r#"{"schema_version":"3","version":"v1","updated":"v1","aliases":{},"prices":[{"provider":"   ","model":"m","currency":"USD","effective_date":"2099-01-01","fast_multiplier":null,"tiers":[{"from_tokens":0,"input_per_million":1.0,"output_per_million":2.0,"cached_input_per_million":null,"cache_write_per_million":null,"cache_storage_per_million_hour":null,"reasoning_per_million":null}]}]}"#;
        let result = parse_catalog_json(json);
        match result {
            Err(ReloadResult::Invalid { reason }) => {
                assert!(reason.contains("empty provider"), "got: {reason}");
            }
            other => panic!("expected Invalid, got {:?}", other),
        }
    }

    #[test]
    fn validate_rejects_uppercase_provider() {
        let json = r#"{"schema_version":"3","version":"v1","updated":"v1","aliases":{},"prices":[{"provider":"Anthropic","model":"m","currency":"USD","effective_date":"2099-01-01","fast_multiplier":null,"tiers":[{"from_tokens":0,"input_per_million":1.0,"output_per_million":2.0,"cached_input_per_million":null,"cache_write_per_million":null,"cache_storage_per_million_hour":null,"reasoning_per_million":null}]}]}"#;
        let result = parse_catalog_json(json);
        match result {
            Err(ReloadResult::Invalid { reason }) => {
                assert!(reason.contains("must be lowercase"), "got: {reason}");
            }
            other => panic!("expected Invalid, got {:?}", other),
        }
    }

    #[test]
    fn validate_rejects_non_usd_currency() {
        let json = r#"{"schema_version":"3","version":"v1","updated":"v1","aliases":{},"prices":[{"provider":"test","model":"m","currency":"EUR","effective_date":"2099-01-01","fast_multiplier":null,"tiers":[{"from_tokens":0,"input_per_million":1.0,"output_per_million":2.0,"cached_input_per_million":null,"cache_write_per_million":null,"cache_storage_per_million_hour":null,"reasoning_per_million":null}]}]}"#;
        let result = parse_catalog_json(json);
        match result {
            Err(ReloadResult::Invalid { reason }) => {
                assert!(reason.contains("currency must be 'USD'"), "got: {reason}");
                assert!(reason.contains("EUR"), "got: {reason}");
            }
            other => panic!("expected Invalid, got {:?}", other),
        }
    }

    #[test]
    fn validate_rejects_negative_required_tier_field() {
        let json = r#"{"schema_version":"3","version":"v1","updated":"v1","aliases":{},"prices":[{"provider":"test","model":"m","currency":"USD","effective_date":"2099-01-01","fast_multiplier":null,"tiers":[{"from_tokens":0,"input_per_million":-1.0,"output_per_million":2.0,"cached_input_per_million":null,"cache_write_per_million":null,"cache_storage_per_million_hour":null,"reasoning_per_million":null}]}]}"#;
        let result = parse_catalog_json(json);
        match result {
            Err(ReloadResult::Invalid { reason }) => {
                assert!(reason.contains("input_per_million"), "got: {reason}");
                assert!(reason.contains("must be >= 0"), "got: {reason}");
            }
            other => panic!("expected Invalid, got {:?}", other),
        }
    }

    #[test]
    fn validate_rejects_nan_required_tier_field() {
        let json = r#"{"schema_version":"3","version":"v1","updated":"v1","aliases":{},"prices":[{"provider":"test","model":"m","currency":"USD","effective_date":"2099-01-01","fast_multiplier":null,"tiers":[{"from_tokens":0,"input_per_million":1.0,"output_per_million":NaN,"cached_input_per_million":null,"cache_write_per_million":null,"cache_storage_per_million_hour":null,"reasoning_per_million":null}]}]}"#;
        let result: Result<PriceCatalog, _> = serde_json::from_str(json);
        // serde_json rejects NaN by default — confirms the type guard holds.
        assert!(result.is_err());
    }

    #[test]
    fn validate_rejects_negative_output_per_million() {
        let json = r#"{"schema_version":"3","version":"v1","updated":"v1","aliases":{},"prices":[{"provider":"test","model":"m","currency":"USD","effective_date":"2099-01-01","fast_multiplier":null,"tiers":[{"from_tokens":0,"input_per_million":1.0,"output_per_million":-0.5,"cached_input_per_million":null,"cache_write_per_million":null,"cache_storage_per_million_hour":null,"reasoning_per_million":null}]}]}"#;
        let result = parse_catalog_json(json);
        match result {
            Err(ReloadResult::Invalid { reason }) => {
                assert!(reason.contains("output_per_million"), "got: {reason}");
            }
            other => panic!("expected Invalid, got {:?}", other),
        }
    }

    #[test]
    fn validate_rejects_negative_optional_cached_input() {
        let json = r#"{"schema_version":"3","version":"v1","updated":"v1","aliases":{},"prices":[{"provider":"test","model":"m","currency":"USD","effective_date":"2099-01-01","fast_multiplier":null,"tiers":[{"from_tokens":0,"input_per_million":1.0,"output_per_million":2.0,"cached_input_per_million":-0.1,"cache_write_per_million":null,"cache_storage_per_million_hour":null,"reasoning_per_million":null}]}]}"#;
        let result = parse_catalog_json(json);
        match result {
            Err(ReloadResult::Invalid { reason }) => {
                assert!(reason.contains("cached_input_per_million"), "got: {reason}");
                assert!(reason.contains("if set"), "got: {reason}");
            }
            other => panic!("expected Invalid, got {:?}", other),
        }
    }

    #[test]
    fn validate_rejects_negative_optional_cache_write() {
        let json = r#"{"schema_version":"3","version":"v1","updated":"v1","aliases":{},"prices":[{"provider":"test","model":"m","currency":"USD","effective_date":"2099-01-01","fast_multiplier":null,"tiers":[{"from_tokens":0,"input_per_million":1.0,"output_per_million":2.0,"cached_input_per_million":null,"cache_write_per_million":-3.0,"cache_storage_per_million_hour":null,"reasoning_per_million":null}]}]}"#;
        let result = parse_catalog_json(json);
        match result {
            Err(ReloadResult::Invalid { reason }) => {
                assert!(reason.contains("cache_write_per_million"), "got: {reason}");
            }
            other => panic!("expected Invalid, got {:?}", other),
        }
    }

    #[test]
    fn validate_rejects_negative_optional_cache_storage() {
        let json = r#"{"schema_version":"3","version":"v1","updated":"v1","aliases":{},"prices":[{"provider":"test","model":"m","currency":"USD","effective_date":"2099-01-01","fast_multiplier":null,"tiers":[{"from_tokens":0,"input_per_million":1.0,"output_per_million":2.0,"cached_input_per_million":null,"cache_write_per_million":null,"cache_storage_per_million_hour":-0.5,"reasoning_per_million":null}]}]}"#;
        let result = parse_catalog_json(json);
        match result {
            Err(ReloadResult::Invalid { reason }) => {
                assert!(
                    reason.contains("cache_storage_per_million_hour"),
                    "got: {reason}"
                );
            }
            other => panic!("expected Invalid, got {:?}", other),
        }
    }

    #[test]
    fn validate_rejects_negative_optional_reasoning() {
        let json = r#"{"schema_version":"3","version":"v1","updated":"v1","aliases":{},"prices":[{"provider":"test","model":"m","currency":"USD","effective_date":"2099-01-01","fast_multiplier":null,"tiers":[{"from_tokens":0,"input_per_million":1.0,"output_per_million":2.0,"cached_input_per_million":null,"cache_write_per_million":null,"cache_storage_per_million_hour":null,"reasoning_per_million":-2.0}]}]}"#;
        let result = parse_catalog_json(json);
        match result {
            Err(ReloadResult::Invalid { reason }) => {
                assert!(reason.contains("reasoning_per_million"), "got: {reason}");
            }
            other => panic!("expected Invalid, got {:?}", other),
        }
    }

    #[test]
    #[serial]
    fn estimate_cost_usd_uses_global_catalog_for_unknown_model() {
        // Reset to embedded catalog and confirm estimate_cost_usd resolves None
        // for an unknown model (does not block ingestion).
        reset_catalog();
        init_catalog(None::<&std::path::Path>);
        let usage = TokenUsage {
            input_tokens: 1000,
            output_tokens: 500,
            cached_input_tokens: 0,
            cache_creation_tokens: 0,
            reasoning_tokens: 0,
        };
        let result = estimate_cost_usd(
            "definitely-not-a-real-model-xyz",
            usage,
            None,
            None,
            CostMode::Calculate,
        );
        assert!(
            result.is_none(),
            "expected None for unknown model, got {:?}",
            result
        );
    }

    #[test]
    #[serial]
    fn estimate_cost_usd_uses_source_cost_in_auto_mode() {
        // In Auto mode, when source_cost is present it is preferred.
        reset_catalog();
        init_catalog(None::<&std::path::Path>);
        let catalog = load_catalog();
        // Find a real model from the embedded catalog so resolve_model succeeds
        // through the global path.
        let model_name = catalog.prices[0].model.clone();
        drop(catalog);
        let usage = TokenUsage {
            input_tokens: 1000,
            output_tokens: 500,
            cached_input_tokens: 0,
            cache_creation_tokens: 0,
            reasoning_tokens: 0,
        };
        let result = estimate_cost_usd(&model_name, usage, Some(0.42), None, CostMode::Auto);
        assert_eq!(result, Some(0.42));
    }

    #[test]
    #[serial]
    fn estimate_cost_usd_display_mode_returns_zero_when_no_source_cost() {
        // In Display mode with no source_cost, returns Some(0.0) regardless of usage.
        reset_catalog();
        init_catalog(None::<&std::path::Path>);
        let catalog = load_catalog();
        let model_name = catalog.prices[0].model.clone();
        drop(catalog);
        let usage = TokenUsage {
            input_tokens: 1000,
            output_tokens: 500,
            cached_input_tokens: 0,
            cache_creation_tokens: 0,
            reasoning_tokens: 0,
        };
        let result = estimate_cost_usd(&model_name, usage, None, None, CostMode::Display);
        assert_eq!(result, Some(0.0));
    }

    #[test]
    #[serial]
    fn init_catalog_falls_back_when_file_invalid_json() {
        // init_catalog with an unparseable file should fall back to the embedded
        // catalog rather than panicking. Covers the warning-emitting branch.
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("bad-catalog.json");
        std::fs::write(&path, "not valid json{{{").unwrap();
        reset_catalog();
        init_catalog(Some(&path));
        // Embedded catalog must be loaded.
        let catalog = load_catalog();
        assert!(!catalog.prices.is_empty());
        assert_eq!(catalog.schema_version, "3");
        // File state should remain unset (embedded fallback path).
        assert_eq!(LAST_FILE_LEN.load(Ordering::SeqCst), 0);
        reset_catalog();
        init_catalog(None::<&std::path::Path>);
    }

    #[test]
    #[serial]
    fn init_catalog_falls_back_when_validation_fails() {
        // init_catalog with a structurally-valid but semantically invalid catalog
        // (empty prices) should fall back to the embedded catalog.
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("invalid-catalog.json");
        let invalid =
            r#"{"schema_version":"3","version":"v","updated":"v","aliases":{},"prices":[]}"#;
        std::fs::write(&path, invalid).unwrap();
        reset_catalog();
        init_catalog(Some(&path));
        let catalog = load_catalog();
        assert!(
            !catalog.prices.is_empty(),
            "embedded fallback should be loaded"
        );
        reset_catalog();
        init_catalog(None::<&std::path::Path>);
    }

    #[test]
    #[serial]
    fn init_catalog_falls_back_on_io_error_for_directory_path() {
        // init_catalog with a directory (not a file) should treat it as an I/O error
        // and fall back to the embedded catalog.
        let tmp = tempfile::tempdir().unwrap();
        let dir_path = tmp.path().join("a-directory");
        std::fs::create_dir_all(&dir_path).unwrap();
        reset_catalog();
        init_catalog(Some(&dir_path));
        let catalog = load_catalog();
        assert!(
            !catalog.prices.is_empty(),
            "embedded fallback should be loaded on IO error"
        );
        reset_catalog();
        init_catalog(None::<&std::path::Path>);
    }

    #[test]
    #[serial]
    fn init_catalog_handles_missing_file_gracefully() {
        // init_catalog with a nonexistent file path is not an error — embedded
        // catalog is used.
        let missing = std::path::Path::new("/tmp/busytok-test-missing-catalog-xyz.json");
        let _ = std::fs::remove_file(missing);
        reset_catalog();
        init_catalog(Some(missing));
        let catalog = load_catalog();
        assert!(!catalog.prices.is_empty());
        reset_catalog();
        init_catalog(None::<&std::path::Path>);
    }
}
