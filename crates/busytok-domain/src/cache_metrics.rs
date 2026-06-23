//! Unified, cross-provider cache metrics. Provider semantics differ ONLY here.

use serde::{Deserialize, Serialize};

/// How a provider reports token fields in its raw payload. Provider
/// differences are confined to this discriminator; everything downstream
/// consumes [`UnifiedCacheMetrics`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderPayloadShape {
    /// OpenAI Codex: `input_tokens` is the full prompt input; the cached alias
    /// is the cache-hit portion of it. No cache-write concept.
    Codex,
    /// Anthropic-native (Claude): `input_tokens` already INCLUDES the
    /// cache-read and cache-creation prompt tokens.
    AnthropicNative,
    /// Anthropic-format but non-Anthropic semantics (DeepSeek, GLM, ...):
    /// `input_tokens` is the NON-cached prompt input only; cache-read +
    /// cache-creation are separate and may exceed `input_tokens`.
    AnthropicCompatibleNonCachedInput,
}

impl ProviderPayloadShape {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::AnthropicNative => "anthropic_native",
            Self::AnthropicCompatibleNonCachedInput => "anthropic_compatible_non_cached_input",
        }
    }

    pub fn parse(s: &str) -> Self {
        match s {
            "codex" => Self::Codex,
            "anthropic_native" => Self::AnthropicNative,
            _ => Self::AnthropicCompatibleNonCachedInput,
        }
    }
}

/// Unified, cross-provider product metrics. Built by adapters from raw audit
/// fields; consumed unchanged by store / runtime / API / frontend.
///
/// Invariant: `cache_read + cache_write + non_cached == prompt_input_total`,
/// with every component `>= 0`. A violation signals a provider-semantic anomaly
/// (e.g. a compatible payload misread as native); the offending record must
/// surface `cache_hit_rate = null` and emit a diagnostic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct UnifiedCacheMetrics {
    /// Total prompt input INCLUDING the cacheable portion.
    pub prompt_input_total_tokens: i64,
    /// Prompt input that was NOT served from cache.
    pub prompt_input_non_cached_tokens: i64,
    /// Prompt input served from cache (cache hit).
    pub cache_read_tokens: i64,
    /// Prompt input written to cache (cache fill).
    pub cache_write_tokens: i64,
}

impl UnifiedCacheMetrics {
    /// Map raw provider audit fields into unified metrics. Inputs are the raw
    /// provider values (already `unwrap_or(0)`'d by the adapter).
    pub fn from_raw(
        shape: ProviderPayloadShape,
        raw_input_tokens: i64,
        cache_read_tokens: i64,
        cache_creation_tokens: i64,
    ) -> Self {
        let read = cache_read_tokens;
        let write = cache_creation_tokens;
        match shape {
            ProviderPayloadShape::Codex => Self {
                prompt_input_total_tokens: raw_input_tokens,
                prompt_input_non_cached_tokens: raw_input_tokens - read,
                cache_read_tokens: read,
                cache_write_tokens: 0,
            },
            ProviderPayloadShape::AnthropicNative => Self {
                prompt_input_total_tokens: raw_input_tokens,
                // Negative when a compatible payload is misread as native —
                // caught by `invariant_holds`.
                prompt_input_non_cached_tokens: raw_input_tokens - read - write,
                cache_read_tokens: read,
                cache_write_tokens: write,
            },
            ProviderPayloadShape::AnthropicCompatibleNonCachedInput => Self {
                prompt_input_non_cached_tokens: raw_input_tokens,
                prompt_input_total_tokens: raw_input_tokens + read + write,
                cache_read_tokens: read,
                cache_write_tokens: write,
            },
        }
    }

    /// True when all components are `>= 0` and the additive identity holds.
    pub fn invariant_holds(&self) -> bool {
        self.prompt_input_total_tokens >= 0
            && self.prompt_input_non_cached_tokens >= 0
            && self.cache_read_tokens >= 0
            && self.cache_write_tokens >= 0
            && self.cache_read_tokens
                + self.cache_write_tokens
                + self.prompt_input_non_cached_tokens
                == self.prompt_input_total_tokens
    }
}

/// Product cache-hit rate in `[0.0, 1.0]`, or `None` when the denominator is
/// zero or the invariant is violated (record must then surface as `null` /
/// `--`; a diagnostic is emitted by the store write path).
pub fn cache_hit_rate(m: UnifiedCacheMetrics) -> Option<f64> {
    if !m.invariant_holds() || m.prompt_input_total_tokens == 0 {
        return None;
    }
    let rate = m.cache_read_tokens as f64 / m.prompt_input_total_tokens as f64;
    (0.0..=1.0).contains(&rate).then_some(rate)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codex_normal_hit_is_in_range() {
        // input=1000 total prompt, 800 served from cache.
        let m = UnifiedCacheMetrics::from_raw(ProviderPayloadShape::Codex, 1000, 800, 0);
        assert!(m.invariant_holds());
        assert_eq!(m.prompt_input_non_cached_tokens, 200);
        let rate = cache_hit_rate(m).unwrap();
        assert!(rate >= 0.0 && rate <= 1.0);
        assert!((rate - 0.8).abs() < 1e-9);
    }

    #[test]
    fn anthropic_native_total_includes_cache() {
        // input_tokens already includes cache_read + cache_creation.
        let m =
            UnifiedCacheMetrics::from_raw(ProviderPayloadShape::AnthropicNative, 1500, 800, 200);
        assert!(m.invariant_holds());
        assert_eq!(m.prompt_input_total_tokens, 1500);
        assert_eq!(m.prompt_input_non_cached_tokens, 500);
        assert!((cache_hit_rate(m).unwrap() - (800.0 / 1500.0)).abs() < 1e-9);
    }

    #[test]
    fn deepseek_compatible_small_input_large_cache_stays_under_100() {
        // raw input_tokens is non-cached only; cache_read dwarfs it.
        let m = UnifiedCacheMetrics::from_raw(
            ProviderPayloadShape::AnthropicCompatibleNonCachedInput,
            10,
            990,
            0,
        );
        assert!(m.invariant_holds());
        assert_eq!(m.prompt_input_total_tokens, 1000);
        let rate = cache_hit_rate(m).unwrap();
        assert!(rate <= 1.0, "rate must not exceed 1.0, got {rate}");
        assert!((rate - 0.99).abs() < 1e-9);
    }

    #[test]
    fn anomalous_native_where_cache_exceeds_input_is_null() {
        // A compatible payload misclassified as native: input(10) < read+write(990).
        let m = UnifiedCacheMetrics::from_raw(ProviderPayloadShape::AnthropicNative, 10, 800, 200);
        assert!(
            !m.invariant_holds(),
            "negative non_cached must violate invariant"
        );
        assert!(cache_hit_rate(m).is_none());
    }

    #[test]
    fn zero_total_yields_no_rate_but_no_anomaly() {
        let m = UnifiedCacheMetrics::from_raw(ProviderPayloadShape::Codex, 0, 0, 0);
        assert!(m.invariant_holds());
        assert!(cache_hit_rate(m).is_none());
    }
}
