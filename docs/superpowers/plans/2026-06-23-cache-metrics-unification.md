# Cache Metrics Unification Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `cache_hit_rate` a cross-provider-consistent, explainable product metric that can never exceed 100%, by splitting token data into a raw-audit layer and a unified-metrics layer with provider differences confined to the adapter mapping.

**Architecture:** A new pure domain module (`cache_metrics.rs`) defines `ProviderPayloadShape`, `UnifiedCacheMetrics`, and an invariant-guarded `cache_hit_rate()`. Unified metrics are derived at every `NormalizedUsageEvent` construction site via that one shared function — today two sites: the Claude adapter (`claude.rs`) and the Codex delta builder (`runtime/scan.rs`, because Codex cumulative→delta conversion needs runtime state). Provider-specific knowledge is confined to determining the shape and raw values at those sites; the mapping rule itself is single-sourced in the domain. **`total_tokens` is NOT touched** — it keeps its existing audit semantics; unified fields serve only hit-rate and breakdown. Because the project is pre-launch with no compatibility requirement, existing dev DBs are reset (delete the SQLite file → relaunch rescans logs and rebuilds), so no stale zero-default rows enter aggregates. The invariant check + diagnostic emission is centralized in the **store write path** (one chokepoint for both providers). DTOs keep raw audit fields and add unified fields alongside (product UI shows unified; raw stays visible in technical-details). Scope: Activity + Overview (per-event rate) + Session/Model detail (aggregate breakdown); Project detail stays totals-only.

**Tech Stack:** Rust workspace (crates: `busytok-domain`, `busytok-adapters`, `busytok-runtime`, `busytok-store`, `busytok-protocol`), SQLite (hand-rolled migrations), React + TypeScript + vitest frontend, `ts-rs` generated DTO types.

## Global Constraints

- `cache_hit_rate` is in `[0.0, 1.0]` or `null` — never above 100%.
- **`total_tokens` is OUT OF SCOPE.** It keeps its existing audit semantics (`input + output + cache_creation + cache_read` for Claude). Unified fields serve only hit-rate and breakdown; they MUST NOT redefine the audit total.
- Unified metrics are derived at every `NormalizedUsageEvent` construction site via the shared `UnifiedCacheMetrics::from_raw`. Two sites today (Claude adapter `claude.rs`; Codex delta builder `runtime/scan.rs`); provider-specific knowledge there is shape + raw values only. Store/runtime-supervisor/API/frontend consume unified fields and never re-derive provider semantics.
- DTOs keep raw audit fields (including `cached_input_tokens`) AND add unified fields. Product UI shows unified by default; raw stays visible in a technical-details/debug area.
- `cached_input_tokens` is demoted to raw/intermediate — never a rate denominator.
- **No compatibility layer, no backfill.** Existing dev DBs are RESET (delete the SQLite file; busytok rescans logs and rebuilds all tables on relaunch). No stale zero-default rows may enter aggregates.
- Every unified-metrics record must satisfy all components `>= 0` and `cache_read + cache_write + non_cached == prompt_input_total`. The check runs **once, in the store write path**; on violation `cache_hit_rate = null` and a structured diagnostic is persisted via the existing diagnostic-persistence path. Adapters do not assemble diagnostics. The per-event `cache_metric` diagnostic reflects **current** state — upserted on violation (stable id, no duplicates on repeated violations) and **deleted when the same event id is later rewritten valid**, so a diagnostic exists ⇔ the event currently violates (no stale warnings).
- `formatCacheHitRate(null)` already renders `"--"` — do not add client-side clamping.

---

## File Structure

**Create:**
- `crates/busytok-domain/src/cache_metrics.rs` — pure unified-metrics core (shape enum, `UnifiedCacheMetrics`, `cache_hit_rate`).
- `crates/busytok-store/migrations/0002_cache_metrics.sql` — additive columns on `usage_events`.

**Modify (domain):**
- `crates/busytok-domain/src/events.rs` — add 3 fields to `NormalizedUsageEvent` + `minimal_for_test`.
- `crates/busytok-domain/src/lib.rs` — `pub mod cache_metrics;`.

**Modify (adapters):**
- `crates/busytok-adapters/src/claude.rs` — classify shape + populate unified fields (no diagnostic emission).
- `crates/busytok-runtime/src/scan.rs` — populate unified fields for the Codex delta builder (no diagnostic emission).

**Modify (store):**
- `crates/busytok-store/src/schema.rs` — register v2 migration, bump `SCHEMA_VERSION`.
- `crates/busytok-store/src/write_queries.rs` — add columns to both SQL consts + `usage_event_params!` macro; centralize the invariant check + diagnostic emission in the usage-event persistence path (single chokepoint for both providers).
- `crates/busytok-store/src/db.rs` — extend `row_to_usage_event` column indices; extend test write SQL if it binds positionally.
- `crates/busytok-store/src/read_models.rs` — extend `ActivityListRow`, `ActivityDetailRow`, `ModelTokenBreakdownRow`.
- `crates/busytok-store/src/read_queries.rs` — extend `read_activity_list`, `read_breakdown_activity_list`, `read_client_source_recent_activity`, `read_activity_detail`, `read_model_token_breakdown`.
- `crates/busytok-store/tests/migrations.rs` — update the `len()==1, ver==1` assertion.

**Modify (protocol/DTO):**
- `crates/busytok-protocol/src/dto.rs` — extend `TokenBreakdownDto` (add unified fields + `cache_hit_rate`, keep raw `cached_input_tokens`).
- `packages/busytok-protocol-types/src/generated.ts` — regenerate via `ts-rs`.

**Modify (runtime):**
- `crates/busytok-runtime/src/supervisor.rs` — `activity_item_from_read_row` (new rate), `activity_detail_from_read_row` (new breakdown + rate), model + session breakdown builds.

**Modify (frontend):**
- `apps/gui/src/pages/ActivityPage.tsx` — delete client-side rate recompute; read DTO `cache_hit_rate`; show new breakdown fields.
- `apps/gui/src/pages/SessionsPage.tsx`, `apps/gui/src/pages/ModelsPage.tsx` — show new breakdown fields + aggregate `cache_hit_rate`.

---

## Task 1: Unified metrics core (pure domain logic)

**Files:**
- Create: `crates/busytok-domain/src/cache_metrics.rs`
- Modify: `crates/busytok-domain/src/lib.rs`
- Test: `crates/busytok-domain/src/cache_metrics.rs` (inline `#[cfg(test)]` module)

**Interfaces:**
- Produces: `ProviderPayloadShape` enum (`Codex`, `AnthropicNative`, `AnthropicCompatibleNonCachedInput`) with `as_str()`/`parse()`; `UnifiedCacheMetrics { prompt_input_total_tokens, prompt_input_non_cached_tokens, cache_read_tokens, cache_write_tokens }` with `from_raw(shape, raw_input, cache_read, cache_creation)` and `invariant_holds()`; free fn `cache_hit_rate(UnifiedCacheMetrics) -> Option<f64>`.

- [ ] **Step 1: Write the failing tests**

Append to `crates/busytok-domain/src/cache_metrics.rs` (create file with tests first):

```rust
//! Unified, cross-provider cache metrics. Provider semantics differ ONLY here.

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
        let m = UnifiedCacheMetrics::from_raw(ProviderPayloadShape::AnthropicNative, 1500, 800, 200);
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
        assert!(!m.invariant_holds(), "negative non_cached must violate invariant");
        assert!(cache_hit_rate(m).is_none());
    }

    #[test]
    fn zero_total_yields_no_rate_but_no_anomaly() {
        let m = UnifiedCacheMetrics::from_raw(ProviderPayloadShape::Codex, 0, 0, 0);
        assert!(m.invariant_holds());
        assert!(cache_hit_rate(m).is_none());
    }
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p busytok-domain cache_metrics`
Expected: FAIL — module/functions not defined (compile error).

- [ ] **Step 3: Write the implementation**

Prepend the implementation above the `#[cfg(test)]` module in `crates/busytok-domain/src/cache_metrics.rs`:

```rust
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
```

Register the module in `crates/busytok-domain/src/lib.rs` — add alongside the existing `pub mod events;`:

```rust
pub mod cache_metrics;
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p busytok-domain cache_metrics`
Expected: PASS — 5 tests.

- [ ] **Step 5: Commit**

```bash
git add crates/busytok-domain/src/cache_metrics.rs crates/busytok-domain/src/lib.rs
git commit -m "feat(domain): unified cross-provider cache metrics core + invariant"
```

---

## Task 2: Add unified fields to `NormalizedUsageEvent`

**Files:**
- Modify: `crates/busytok-domain/src/events.rs:31-39` (struct), `:90-98` (`minimal_for_test`)
- Test: existing `cargo test -p busytok-domain` must still compile/pass

**Interfaces:**
- Produces on `NormalizedUsageEvent`: `provider_payload_shape: ProviderPayloadShape`, `prompt_input_total_tokens: i64`, `prompt_input_non_cached_tokens: i64`. Consumes `use crate::cache_metrics::ProviderPayloadShape;`.

- [ ] **Step 1: Write the failing test**

Add to the existing test module in `crates/busytok-domain/src/events.rs` (or the crate's tests). If there is no inline test module, add one:

```rust
#[cfg(test)]
mod unified_field_tests {
    use super::*;
    use crate::cache_metrics::ProviderPayloadShape;

    #[test]
    fn minimal_event_has_default_unified_fields() {
        let e = NormalizedUsageEvent::minimal_for_test("t", crate::AgentKind::ClaudeCode);
        assert_eq!(e.provider_payload_shape, ProviderPayloadShape::Codex);
        assert_eq!(e.prompt_input_total_tokens, 0);
        assert_eq!(e.prompt_input_non_cached_tokens, 0);
    }
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p busytok-domain unified_field_tests`
Expected: FAIL — no such fields (compile error).

- [ ] **Step 3: Write the implementation**

In `crates/busytok-domain/src/events.rs`, add the import near the top:

```rust
use crate::cache_metrics::ProviderPayloadShape;
```

Add three fields to the `NormalizedUsageEvent` struct immediately after `tool_tokens: i64,` (line 39):

```rust
    /// Discriminator recording how the raw provider payload reported tokens.
    /// Provider differences live here; downstream consumes unified fields.
    pub provider_payload_shape: ProviderPayloadShape,
    /// Unified: total prompt input INCLUDING the cacheable portion.
    pub prompt_input_total_tokens: i64,
    /// Unified: prompt input NOT served from cache.
    pub prompt_input_non_cached_tokens: i64,
```

In `minimal_for_test` (around line 98), add after `tool_tokens: 0,`:

```rust
            provider_payload_shape: ProviderPayloadShape::Codex,
            prompt_input_total_tokens: 0,
            prompt_input_non_cached_tokens: 0,
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p busytok-domain`
Expected: PASS. NOTE: other crates referencing `NormalizedUsageEvent { ... }` literally (adapters, tests) will now fail to compile until Tasks 3–4 set the new fields — that is expected; this task's scope is the domain crate only. Run `cargo test -p busytok-domain` (not the whole workspace) to isolate.

- [ ] **Step 5: Commit**

```bash
git add crates/busytok-domain/src/events.rs
git commit -m "feat(domain): add unified cache-metric fields to NormalizedUsageEvent"
```

---

## Task 3: Claude adapter classifies shape + populates unified fields

**Files:**
- Modify: `crates/busytok-adapters/src/claude.rs:118-134` (shape detection), `:221-229` (event field set). **Do NOT touch `total_tokens` (line 149) — it keeps its existing audit semantics.**
- Test: `crates/busytok-adapters/src/claude.rs` inline tests, or `crates/busytok-adapters/tests/`

**Interfaces:**
- Consumes: `busytok_domain::cache_metrics::{ProviderPayloadShape, UnifiedCacheMetrics}`.
- Produces: a `NormalizedUsageEvent` with `provider_payload_shape`, `prompt_input_total_tokens`, `prompt_input_non_cached_tokens` populated. (No diagnostic here — Task 6 centralizes it in the write path.)

- [ ] **Step 1: Write the failing test**

Add (or extend) a test that parses a DeepSeek-style Anthropic-format usage line (small `input_tokens`, large `cache_read_input_tokens`) and asserts: (a) `provider_payload_shape == AnthropicCompatibleNonCachedInput`, (b) `prompt_input_total_tokens == input + cache_read + cache_creation`. Because the adapter's exact parse entry point and fixtures vary, locate the existing Claude adapter test module first (`grep -n "#\[cfg(test)\]" crates/busytok-adapters/src/claude.rs` or `ls crates/busytok-adapters/tests/`) and mirror an existing test's setup. Test body:

```rust
#[test]
fn claude_deepseek_style_payload_maps_to_compatible_shape() {
    let events = parse_claude_usage_line(/* reuse the existing fixture helper with:
        input_tokens: 10, cache_read_input_tokens: 990, cache_creation_input_tokens: 0 */);
    let usage = events
        .iter()
        .filter_map(|e| match e {
            ParsedLogEvent::Normalized(NormalizedEvent::Usage(u)) => Some(u),
            _ => None,
        })
        .next()
        .expect("a usage event");
    assert_eq!(
        usage.provider_payload_shape,
        busytok_domain::cache_metrics::ProviderPayloadShape::AnthropicCompatibleNonCachedInput
    );
    assert_eq!(usage.prompt_input_total_tokens, 1000);
}
```

(Replace `parse_claude_usage_line` / fixture plumbing with the adapter's real test helper. If no fixture helper exists, construct the input JSON the way existing tests do.)

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p busytok-adapters claude_deepseek_style`
Expected: FAIL — `provider_payload_shape` not set (defaults to `Codex`), total wrong.

- [ ] **Step 3: Write the implementation**

In `crates/busytok-adapters/src/claude.rs`, add the import:

```rust
use busytok_domain::cache_metrics::{
    ProviderPayloadShape, UnifiedCacheMetrics,
};
```

Replace the block at lines 118–134 (the `let input_tokens = raw_input;` + `if cache_read + cache_creation > raw_input { debug!(...) }`) with explicit shape detection:

```rust
        // Classify the payload shape. Genuine Anthropic includes cached prompt
        // tokens in input_tokens; DeepSeek/GLM (Anthropic-format, non-Anthropic
        // semantics) report non-cached-only input where cache_read + cache_creation
        // can exceed input_tokens.
        let provider_shape =
            if cache_read_tokens + cache_creation_tokens > raw_input {
                ProviderPayloadShape::AnthropicCompatibleNonCachedInput
            } else {
                ProviderPayloadShape::AnthropicNative
            };
        let input_tokens = raw_input;
        let unified = UnifiedCacheMetrics::from_raw(
            provider_shape,
            raw_input,
            cache_read_tokens,
            cache_creation_tokens,
        );
```

Then in the event literal (lines 221–229), set the new fields alongside the existing token fields. Leave `total_tokens` (line 149) exactly as-is — it is the audit total and is out of scope for this refactor:

```rust
            input_tokens,
            output_tokens,
            total_tokens,
            cached_input_tokens,
            cache_creation_tokens,
            cache_read_tokens,
            provider_payload_shape: provider_shape,
            prompt_input_total_tokens: unified.prompt_input_total_tokens,
            prompt_input_non_cached_tokens: unified.prompt_input_non_cached_tokens,
            reasoning_tokens: 0,
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p busytok-adapters`
Expected: PASS. (Only `UnifiedCacheMetrics` + `ProviderPayloadShape` are imported; `cache_hit_rate` is not needed in the adapter.)

- [ ] **Step 5: Commit**

```bash
git add crates/busytok-adapters/src/claude.rs
git commit -m "feat(adapters/claude): classify payload shape + unified cache metrics"
```

---

## Task 4: Codex delta builder populates unified fields

**Files:**
- Modify: `crates/busytok-runtime/src/scan.rs:899-928` (`build_codex_delta_events` event literal)
- Test: `crates/busytok-runtime/src/scan.rs` tests (locate existing codex delta tests via `grep -n "build_codex_delta" crates/busytok-runtime/src/scan.rs`)

**Interfaces:**
- Consumes: `busytok_domain::cache_metrics::{ProviderPayloadShape, UnifiedCacheMetrics}`.
- Produces: the Codex `NormalizedUsageEvent` with `provider_payload_shape: Codex` and unified fields set. (No diagnostic here — Task 6 centralizes it.)

- [ ] **Step 1: Write the failing test**

Add to the scan.rs test module:

```rust
#[test]
fn codex_delta_event_carries_unified_metrics() {
    // delta_input=1000, delta_cached=800 (cache hit portion of the 1000).
    let events = build_codex_delta_events_for_test(1000, 800, 200, 0, 1400 /* total */);
    let usage = first_usage_event(&events);
    assert_eq!(
        usage.provider_payload_shape,
        busytok_domain::cache_metrics::ProviderPayloadShape::Codex
    );
    assert_eq!(usage.prompt_input_total_tokens, 1000);
    assert_eq!(usage.prompt_input_non_cached_tokens, 200);
}
```

(Use the existing test helper that drives `build_codex_delta_events`; if none, construct a `CodexTokenSnapshot` with `delta_*` fields and call the real function — mirror how current scan.rs tests build snapshots.)

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p busytok-runtime codex_delta_event_carries_unified`
Expected: FAIL — fields not set.

- [ ] **Step 3: Write the implementation**

In `crates/busytok-runtime/src/scan.rs`, add the import near the top:

```rust
use busytok_domain::cache_metrics::{ProviderPayloadShape, UnifiedCacheMetrics};
```

In `build_codex_delta_events`, after `delta_*` locals are computed and before the event literal (around line 899), add:

```rust
            let unified = UnifiedCacheMetrics::from_raw(
                ProviderPayloadShape::Codex,
                delta_input,
                delta_cached,
                0, // Codex has no cache-creation concept
            );
```

In the event literal (lines 920–928), set the new fields:

```rust
                input_tokens: delta_input,
                output_tokens: delta_output,
                total_tokens: delta_total,
                cached_input_tokens: delta_cached,
                cache_creation_tokens: 0,
                cache_read_tokens: delta_cached,
                provider_payload_shape: ProviderPayloadShape::Codex,
                prompt_input_total_tokens: unified.prompt_input_total_tokens,
                prompt_input_non_cached_tokens: unified.prompt_input_non_cached_tokens,
                reasoning_tokens: delta_reasoning,
```

Diagnostic emission for invariant violations is centralized in the write path (Task 6) — `scan.rs` does not assemble diagnostics.

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p busytok-runtime`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/busytok-runtime/src/scan.rs
git commit -m "feat(runtime/scan): codex delta events carry unified cache metrics"
```

---

## Task 5: v2 migration — add columns to `usage_events`

**Files:**
- Create: `crates/busytok-store/migrations/0002_cache_metrics.sql`
- Modify: `crates/busytok-store/src/schema.rs`, `crates/busytok-store/tests/migrations.rs:51-60`
- Test: `crates/busytok-store/tests/migrations.rs`

**Interfaces:**
- Produces: `usage_events` gains `provider_payload_shape TEXT NOT NULL DEFAULT 'codex'`, `prompt_input_total_tokens INTEGER NOT NULL DEFAULT 0`, `prompt_input_non_cached_tokens INTEGER NOT NULL DEFAULT 0`. `SCHEMA_VERSION == 2`, `migrations().len() == 2`.
- **Required upgrade action (no compatibility, per Global Constraints):** existing dev DBs MUST be reset before first run with the new code — delete the SQLite file (path from `busytok-config::paths`). On relaunch busytok rescans agent logs and rebuilds every table with correct unified fields. The defaults exist only so the new columns are non-null during the migration itself; **no stale zero-default rows may remain** to pollute aggregate `SUM(prompt_input_total_tokens)` denominators.

- [ ] **Step 1: Write the failing test**

Update `crates/busytok-store/tests/migrations.rs:51-60` to expect v2:

```rust
#[test]
fn baseline_plus_cache_metrics_migrations() {
    let migs = busytok_store::schema::migrations();
    assert_eq!(migs.len(), 2);
    assert_eq!(busytok_store::schema::SCHEMA_VERSION, 2);
    let mut conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.execute_batch(busytok_store::schema::CREATE_SCHEMA_VERSION_TABLE).unwrap();
    for (_, sql) in &migs {
        conn.execute_batch(sql).unwrap();
    }
    // v2 columns exist on usage_events.
    let cols: Vec<String> = conn
        .prepare("PRAGMA table_info(usage_events)").unwrap()
        .query_map([], |r| r.get::<_, String>(1)).unwrap()
        .filter_map(Result::ok).collect();
    assert!(cols.contains(&"provider_payload_shape".to_string()));
    assert!(cols.contains(&"prompt_input_total_tokens".to_string()));
    assert!(cols.contains(&"prompt_input_non_cached_tokens".to_string()));
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p busytok-store --test migrations`
Expected: FAIL — `migrations().len() == 1`, `SCHEMA_VERSION == 1`, columns absent.

- [ ] **Step 3: Write the implementation**

Create `crates/busytok-store/migrations/0002_cache_metrics.sql`:

```sql
-- v2: unified cache-metrics columns on usage_events. Additive; NO backfill and
-- NO stale-row mixing — existing dev DBs are RESET (delete the SQLite file;
-- busytok rescans logs on relaunch and rebuilds all rows with correct unified
-- fields). DEFAULT 'codex' matches NormalizedUsageEvent::minimal_for_test; the
-- defaults exist only so the columns are non-null during migration itself.
ALTER TABLE usage_events ADD COLUMN provider_payload_shape TEXT NOT NULL DEFAULT 'codex';
ALTER TABLE usage_events ADD COLUMN prompt_input_total_tokens INTEGER NOT NULL DEFAULT 0;
ALTER TABLE usage_events ADD COLUMN prompt_input_non_cached_tokens INTEGER NOT NULL DEFAULT 0;
```

In `crates/busytok-store/src/schema.rs`:

```rust
pub const SCHEMA_VERSION: u32 = 2;

pub const BASELINE_SQL: &str = include_str!("../migrations/0001_baseline.sql");
const CACHE_METRICS_SQL: &str = include_str!("../migrations/0002_cache_metrics.sql");

pub fn migrations() -> Vec<(u32, &'static str)> {
    vec![(1, BASELINE_SQL), (2, CACHE_METRICS_SQL)]
}
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p busytok-store --test migrations`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/busytok-store/migrations/0002_cache_metrics.sql crates/busytok-store/src/schema.rs crates/busytok-store/tests/migrations.rs
git commit -m "feat(store): v2 migration adds unified cache-metric columns"
```

---

## Task 6: Store write path — bind new columns + centralize invariant diagnostic

**Files:**
- Modify: `crates/busytok-store/src/write_queries.rs:17-135` (both SQL consts + `usage_event_params!` macro), the usage-event persistence function (`upsert_usage_events_dedup_aware`, `:247`), tail-replay path `:999-1044`
- Modify: `crates/busytok-store/src/db.rs:84-142` (test write SQL) IF it is used by tests that assert positional binding; otherwise extend for consistency.
- Test: `crates/busytok-store` existing write tests must pass; add one asserting a round-tripped event preserves the new fields, and one asserting an invariant violation records a diagnostic.

**Interfaces:**
- Produces: `USAGE_INSERT_IGNORE_SQL` / `USAGE_UPSERT_BY_ID_SQL` include the 3 new columns (positions `?44..=?46`); `usage_event_params!` binds `$event.provider_payload_shape.as_str()`, `$event.prompt_input_total_tokens`, `$event.prompt_input_non_cached_tokens`.
- Produces: in the usage-event persistence path, after each event is written, the store syncs a per-event `cache_metric` diagnostic to the event's CURRENT invariant state — upsert on violation (stable id `cache-metric-violation:{event.id}`, no duplicates), delete on a valid rewrite (recovery). This is the single chokepoint covering both Claude and Codex events; `source_id` is left empty (source attribution via `source_file_id`/`source_path`/`source_line`, matching the existing diagnostic convention; the event id lives in the diagnostic `id` + `detail_json`).

- [ ] **Step 1: Write the failing test**

In `crates/busytok-store` (locate the existing usage write/read round-trip test, e.g. via `grep -rn "write_usage_event" crates/busytok-store/tests/`), add:

```rust
#[test]
fn usage_event_round_trips_unified_cache_fields() {
    let db = busytok_store::Database::open_in_memory().unwrap();
    let mut e = busytok_domain::NormalizedUsageEvent::minimal_for_test("u1", busytok_domain::AgentKind::ClaudeCode);
    e.provider_payload_shape = busytok_domain::cache_metrics::ProviderPayloadShape::AnthropicCompatibleNonCachedInput;
    e.prompt_input_total_tokens = 1000;
    e.prompt_input_non_cached_tokens = 10;
    e.cache_read_tokens = 990;
    db.write_usage_event(&e, "gen1", busytok_store::WritePolicy::Replace, None).unwrap();
    let got = db.get_usage_event("u1").unwrap().unwrap();
    assert_eq!(got.prompt_input_total_tokens, 1000);
    assert_eq!(got.prompt_input_non_cached_tokens, 10);
    assert_eq!(got.provider_payload_shape, e.provider_payload_shape);
}
```

(Use the real `Database` write/read API names; if `write_usage_event` / `get_usage_event` signatures differ, mirror an existing round-trip test's calls.)

Then add a second test for the centralized diagnostic:

```rust
#[test]
fn cache_metric_diagnostic_lifecycle_records_and_recovers() {
    let db = busytok_store::Database::open_in_memory().unwrap();
    let mut e = busytok_domain::NormalizedUsageEvent::minimal_for_test("bad", busytok_domain::AgentKind::ClaudeCode);
    // Impossible combination: total(10) < read(800)+write(200)+non_cached(10).
    e.prompt_input_total_tokens = 10;
    e.prompt_input_non_cached_tokens = 10;
    e.cache_read_tokens = 800;
    e.cache_creation_tokens = 200;
    db.write_usage_event(&e, "gen1", busytok_store::WritePolicy::Replace, None).unwrap();
    let diags = db.list_diagnostic_events(/* existing accessor */).unwrap();
    assert!(diags.iter().any(|d| d.category == "cache_metric"));

    // Same event id rewritten with a valid combination ⇒ diagnostic is cleared
    // (no stale warning). total(1010) == non_cached(10)+read(800)+write(200).
    e.prompt_input_total_tokens = 1010;
    db.write_usage_event(&e, "gen1", busytok_store::WritePolicy::Replace, None).unwrap();
    let diags = db.list_diagnostic_events(/* existing accessor */).unwrap();
    assert!(
        diags.iter().all(|d| d.category != "cache_metric"),
        "recovered event must leave no stale warning"
    );
}
```

(Match the real diagnostic-list accessor name; mirror an existing diagnostic test.)

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p busytok-store usage_event_round_trips_unified`
Expected: FAIL — columns not written / read back.

- [ ] **Step 3: Write the implementation**

In `USAGE_INSERT_IGNORE_SQL` and `USAGE_UPSERT_BY_ID_SQL` (`write_queries.rs:17-60`), extend the column list and value placeholders. After `is_sidechain` in the column list add `, provider_payload_shape, prompt_input_total_tokens, prompt_input_non_cached_tokens`, and extend the `VALUES (...)` placeholder run from `?41, ?42, ?43` to `?41, ?42, ?43, ?44, ?45, ?46`.

In the `ON CONFLICT(id) DO UPDATE SET` block of `USAGE_UPSERT_BY_ID_SQL`, add:

```sql
        provider_payload_shape = excluded.provider_payload_shape, \
        prompt_input_total_tokens = excluded.prompt_input_total_tokens, \
        prompt_input_non_cached_tokens = excluded.prompt_input_non_cached_tokens, \
```

In the `usage_event_params!` macro (`write_queries.rs:87-133`), after `$event.is_sidechain,` add:

```rust
            $event.provider_payload_shape.as_str(),
            $event.prompt_input_total_tokens,
            $event.prompt_input_non_cached_tokens,
```

Extend the tail-replay positional bind at `write_queries.rs:999-1044` (`apply_single_replay`) to bind the 3 new columns (default `'codex'`, `0`, `0` when absent from replay JSON) so the column count matches the SQL.

Then add the **centralized invariant check + diagnostic**. This is the single chokepoint covering both Claude and Codex events (Finding 5). Add a helper in `write_queries.rs`:

```rust
/// Keep the per-event `cache_metric` diagnostic in sync with the event's
/// CURRENT unified-metric state. Called from the usage-event persistence path
/// for every event, regardless of provider.
///
/// Lifecycle (recovery semantics): the write path upserts the same event id
/// (Replace / dedupe-aware). On violation, UPSERT the diagnostic (stable id →
/// no duplicates on repeated violations of the same event). On a later VALID
/// rewrite of the same event id, DELETE the prior diagnostic so a recovered
/// event leaves no stale warning. Contract: a `cache_metric` diagnostic exists
/// ⇔ the event currently violates the invariant.
fn sync_cache_metric_diagnostic(
    conn: &rusqlite::Connection,
    event: &busytok_domain::NormalizedUsageEvent,
) -> rusqlite::Result<()> {
    let metrics = busytok_domain::cache_metrics::UnifiedCacheMetrics {
        prompt_input_total_tokens: event.prompt_input_total_tokens,
        prompt_input_non_cached_tokens: event.prompt_input_non_cached_tokens,
        cache_read_tokens: event.cache_read_tokens,
        cache_write_tokens: event.cache_creation_tokens,
    };
    let diag_id = format!("cache-metric-violation:{}", event.id);
    if metrics.invariant_holds() {
        // Recovery: a prior violating write of this event id is now valid.
        return delete_diagnostic_by_id(conn, &diag_id);
    }
    let diag = busytok_domain::OperationalDiagnosticEvent {
        id: diag_id,
        agent: Some(event.agent),
        // source_id is SOURCE-level, not the usage event id — matches the
        // existing convention (parse-error diagnostics use an empty source_id
        // and attribute via source_file_id/source_path/source_line). The event
        // id is preserved in the diagnostic `id` and detail_json for traceability.
        source_id: Some(String::new()),
        source_file_id: Some(event.source_file_id.clone()),
        source_path: Some(event.source_path.clone()),
        source_line: Some(event.source_line as i64),
        category: "cache_metric".to_string(),
        severity: "warning".to_string(),
        message: "cache metric invariant violated; cache_hit_rate nulled".to_string(),
        detail_json: Some(format!(
            "{{\"event_id\":\"{}\",\"shape\":\"{}\",\"total\":{},\"non_cached\":{},\"cache_read\":{},\"cache_write\":{}}}",
            event.id,
            event.provider_payload_shape.as_str(),
            event.prompt_input_total_tokens,
            event.prompt_input_non_cached_tokens,
            event.cache_read_tokens,
            event.cache_creation_tokens,
        )),
        happened_at_ms: event.timestamp_ms,
        created_at_ms: busytok_domain::now_ms(),
    };
    upsert_diagnostic_event(conn, &diag)
}
```

Call `sync_cache_metric_diagnostic(conn, event)` for each event after its upsert inside `upsert_usage_events_dedup_aware` (write_queries.rs:247), using the same `conn`. The two persistence primitives it calls must be wired to the existing diagnostic table:
- `upsert_diagnostic_event(conn, &diag)` — `INSERT OR REPLACE` on the diagnostics table (stable `id` ⇒ idempotent on repeated violations of the same event). If the existing `record_diagnostic_event` is a plain `INSERT`, add an `INSERT OR REPLACE` variant (or change it to upsert).
- `delete_diagnostic_by_id(conn, &id)` — `DELETE FROM <diagnostics_table> WHERE id = ?1` (the recovery path; cheap PK delete).
Locate the diagnostics table + existing helper with `grep -rn "fn record_diagnostic_event\|diagnostic" crates/busytok-store/src/` (in `write_queries.rs` or `writer.rs`). This reuses real infra; no second emission path.

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p busytok-store`
Expected: PASS (including the new round-trip test).

- [ ] **Step 5: Commit**

```bash
git add crates/busytok-store/src/write_queries.rs crates/busytok-store/src/db.rs
git commit -m "feat(store): write path persists unified cache-metric columns"
```

---

## Task 7: Store read path — read models + queries

**Files:**
- Modify: `crates/busytok-store/src/db.rs:1768-1820` (`row_to_usage_event` indices for the 3 new columns)
- Modify: `crates/busytok-store/src/read_models.rs` — `ActivityListRow`, `ActivityDetailRow`, `ModelTokenBreakdownRow`
- Modify: `crates/busytok-store/src/read_queries.rs` — `read_activity_list`, `read_breakdown_activity_list`, `read_client_source_recent_activity`, `read_activity_detail`, `read_model_token_breakdown`
- Test: existing store tests + a read assertion.

**Interfaces:**
- Produces: the three read-model rows carry `prompt_input_total_tokens`, `prompt_input_non_cached_tokens`, `cache_read_tokens`, `cache_creation_tokens` (alias for `cache_write`); the five SELECTs fetch them.

- [ ] **Step 1: Write the failing test**

Add a store test asserting `read_activity_list` rows expose the unified fields (build on the round-trip fixture from Task 6; insert an event with known unified fields, then read the activity list and assert the row's `prompt_input_total_tokens` / `cache_read_tokens`).

```rust
#[test]
fn activity_list_row_exposes_unified_fields() {
    let db = busytok_store::Database::open_in_memory().unwrap();
    let mut e = busytok_domain::NormalizedUsageEvent::minimal_for_test("u1", busytok_domain::AgentKind::ClaudeCode);
    e.prompt_input_total_tokens = 1000;
    e.cache_read_tokens = 990;
    e.cache_creation_tokens = 0;
    e.timestamp_ms = busytok_domain::now_ms();
    db.write_usage_event(&e, "gen1", busytok_store::WritePolicy::Replace, None).unwrap();
    let rows = busytok_store::read_queries::read_activity_list(/* existing call signature */).unwrap();
    assert_eq!(rows[0].prompt_input_total_tokens, 1000);
    assert_eq!(rows[0].cache_read_tokens, 990);
}
```

(Match the real `read_activity_list` signature — it takes a connection + generation + range + filters; mirror an existing caller.)

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p busytok-store activity_list_row_exposes_unified`
Expected: FAIL — fields absent on the row.

- [ ] **Step 3: Write the implementation**

In `read_models.rs`, add fields to each row:
- `ActivityListRow` (after `cached_input_tokens`): `prompt_input_total_tokens: i64`, `prompt_input_non_cached_tokens: i64`, `cache_read_tokens: i64`, `cache_creation_tokens: i64`.
- `ActivityDetailRow` (already has `cache_creation_tokens`/`cache_read_tokens`): add `prompt_input_total_tokens: i64`, `prompt_input_non_cached_tokens: i64`, `provider_payload_shape: String`.
- `ModelTokenBreakdownRow`: replace/augment so it carries `prompt_input_total_tokens`, `prompt_input_non_cached_tokens`, `cache_read_tokens`, `cache_creation_tokens`, `input_tokens`, `output_tokens`, `cached_input_tokens`, `reasoning_tokens` (sums — `cached_input_tokens` kept as raw audit).

In `read_queries.rs`, extend the SELECT column lists and row binds:
- `read_activity_list` (`:341-437`), `read_breakdown_activity_list` (`:440-496`), `read_client_source_recent_activity` (`:785-825`): add `prompt_input_total_tokens, prompt_input_non_cached_tokens, cache_read_tokens, cache_creation_tokens` to the SELECT, and bind them onto `ActivityListRow` at the row-mapping closure.
- `read_activity_detail` (`:499-564`): add `provider_payload_shape, prompt_input_total_tokens, prompt_input_non_cached_tokens` to the 39-column SELECT and bind onto `ActivityDetailRow` (`:544-546` area).
- `read_model_token_breakdown` (`:828-855`): change the SUMs to `SUM(prompt_input_total_tokens), SUM(prompt_input_non_cached_tokens), SUM(cache_read_tokens), SUM(cache_creation_tokens), SUM(input_tokens), SUM(output_tokens), SUM(cached_input_tokens), SUM(reasoning_tokens)` and bind onto `ModelTokenBreakdownRow`.

In `db.rs:row_to_usage_event` (`:1768-1820`), the full-event SELECT must include the 3 new columns; bind `provider_payload_shape` via `ProviderPayloadShape::parse(row.get::<_, String>(idx))` and the two integer fields at the next indices. Update the column-index comments accordingly.

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p busytok-store`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/busytok-store/src/db.rs crates/busytok-store/src/read_models.rs crates/busytok-store/src/read_queries.rs
git commit -m "feat(store): read models + queries surface unified cache metrics"
```

---

## Task 8: Extend `TokenBreakdownDto` with unified fields; regenerate TS types

**Files:**
- Modify: `crates/busytok-protocol/src/dto.rs:426-433` (`TokenBreakdownDto`); fix the example at `dto.rs:1296` if it constructs the old shape.
- Regenerate: `packages/busytok-protocol-types/src/generated.ts`
- Test: `cargo test -p busytok-protocol`; `pnpm --filter @busytok/protocol-types build` (or the repo's ts-rs generation command — locate via `grep -rn "ts-rs\|export!" crates/busytok-protocol/src/ts.rs`).

**Interfaces:**
- Produces `TokenBreakdownDto` — raw audit fields KEPT, unified fields ADDED alongside (Finding 3: do not collapse the two layers):
  ```rust
  pub struct TokenBreakdownDto {
      // Unified product metrics (shown by default in the UI):
      pub prompt_input_total_tokens: Option<i64>,
      pub prompt_input_non_cached_tokens: Option<i64>,
      pub cache_read_tokens: Option<i64>,
      pub cache_write_tokens: Option<i64>,
      pub cache_hit_rate: Option<f64>,
      // Raw audit fields (kept for technical-details/debug visibility):
      pub input_tokens: Option<i64>,
      pub output_tokens: Option<i64>,
      pub cached_input_tokens: Option<i64>,
      pub reasoning_tokens: Option<i64>,
      pub total_tokens: i64,
  }
  ```
  Product UI renders the unified fields; raw fields remain available in a technical-details area. `cache_hit_rate` added; nothing removed.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn token_breakdown_dto_keeps_raw_and_adds_unified() {
    let tb = TokenBreakdownDto {
        prompt_input_total_tokens: Some(1000),
        prompt_input_non_cached_tokens: Some(10),
        cache_read_tokens: Some(990),
        cache_write_tokens: None,
        cache_hit_rate: Some(0.99),
        input_tokens: Some(10),
        output_tokens: Some(50),
        cached_input_tokens: Some(990),
        reasoning_tokens: None,
        total_tokens: 1050,
    };
    let json = serde_json::to_string(&tb).unwrap();
    // Unified additions:
    assert!(json.contains("prompt_input_total_tokens"));
    assert!(json.contains("cache_hit_rate"));
    // Raw audit field still present (not collapsed away):
    assert!(json.contains("cached_input_tokens"));
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p busytok-protocol token_breakdown_dto_has_unified`
Expected: FAIL — struct still has `cached_input_tokens`, no `cache_hit_rate`.

- [ ] **Step 3: Write the implementation**

Replace the `TokenBreakdownDto` definition at `dto.rs:426-433` with the shape in the Interfaces block above. Update the `dto.rs:1296` example fixture (and any other `TokenBreakdownDto { ... }` literal — find with `grep -rn "TokenBreakdownDto {" crates/`) to the new field set.

Regenerate the TS types by running the project's ts-rs export command (from the repo root, the conventional invocation — confirm in `crates/busytok-protocol` README or `ts.rs`; typically `cargo test -p busytok-protocol export_bindings` or `cargo run` of a small bin). Then verify `packages/busytok-protocol-types/src/generated.ts` shows the new `TokenBreakdownDto` fields and no `cached_input_tokens`.

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p busytok-protocol` then `pnpm typecheck` (in `apps/gui`)
Expected: PASS on Rust; the GUI typecheck will FAIL on the not-yet-updated `ActivityPage.tsx` / breakdown pages — that is expected and resolved in Task 10. Isolate by running `cargo test -p busytok-protocol` only here.

- [ ] **Step 5: Commit**

```bash
git add crates/busytok-protocol/src/dto.rs packages/busytok-protocol-types/src/generated.ts
git commit -m "feat(protocol): TokenBreakdownDto unified fields + cache_hit_rate; keep raw cached_input_tokens"
```

---

## Task 9: Supervisor — unified rate + breakdowns (all three build sites)

**Files:**
- Modify: `crates/busytok-runtime/src/supervisor.rs:964-992` (`activity_item_from_read_row`), `:994-1078` (`activity_detail_from_read_row` incl. `TokenBreakdownDto` at `:1009-1016` and notes `:1024-1038`), `:2403-2421` (model breakdown), `:2588-2594` (session breakdown)
- Test: `crates/busytok-runtime` supervisor tests (locate via `grep -n "activity_item_from_read_row\|cache_hit_rate" crates/busytok-runtime/src/supervisor.rs` and existing tests).

**Interfaces:**
- Consumes: the extended read-model rows (Task 7) + `busytok_domain::cache_metrics::{UnifiedCacheMetrics, cache_hit_rate, ProviderPayloadShape}`.
- Produces: `ActivityListItemDto.cache_hit_rate` from `cache_hit_rate(UnifiedCacheMetrics { … })` reconstructed **directly from the row's persisted unified fields** (`from_raw` is ingest-only; the read path consumes stored fields, never re-deriving shape). `TokenBreakdownDto` populated with unified fields + aggregate `cache_hit_rate` for model/session breakdowns.

- [ ] **Step 1: Write the failing test**

Add a supervisor test (or unit test on a small helper extracted for testability — see Step 3) asserting: given an `ActivityListRow` with `prompt_input_total_tokens=1000`, `cache_read_tokens=990`, the produced `ActivityListItemDto.cache_hit_rate == Some(0.99)`. And a DeepSeek-style row where the OLD formula would exceed 1.0 now yields `<= 1.0`.

```rust
#[test]
fn activity_item_rate_uses_unified_denominator() {
    let row = activity_list_row_fixture(/* prompt_input_total=1000, cache_read=990, shape=compatible */);
    let dto = BusytokSupervisor::activity_item_from_read_row(&row);
    let rate = dto.cache_hit_rate.expect("rate present");
    assert!(rate <= 1.0);
    assert!((rate - 0.99).abs() < 1e-9);
}
```

(If `activity_item_from_read_row` is private and tests are in the same crate, this works directly; otherwise extract the rate computation into a `pub(crate)` helper `fn list_cache_hit_rate(row) -> Option<f64>` and test that.)

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p busytok-runtime activity_item_rate_uses_unified`
Expected: FAIL — rate still `cached_input_tokens / input_tokens`.

- [ ] **Step 3: Write the implementation**

Add a private helper near `activity_item_from_read_row` (supervisor.rs ~963):

```rust
    /// Unified cache-hit rate for a list row, null on invariant violation.
    /// Reads the row's persisted unified fields directly — `from_raw` is
    /// ingest-only; the read path consumes stored fields.
    fn list_cache_hit_rate(row: &busytok_store::read_models::ActivityListRow) -> Option<f64> {
        let m = busytok_domain::cache_metrics::UnifiedCacheMetrics {
            prompt_input_total_tokens: row.prompt_input_total_tokens,
            prompt_input_non_cached_tokens: row.prompt_input_non_cached_tokens,
            cache_read_tokens: row.cache_read_tokens,
            cache_write_tokens: row.cache_creation_tokens,
        };
        busytok_domain::cache_metrics::cache_hit_rate(m)
    }
```

Replace the rate computation at supervisor.rs:968-972 with:

```rust
        let cache_hit_rate = Self::list_cache_hit_rate(item);
```

For `activity_detail_from_read_row`, replace the `TokenBreakdownDto` build (supervisor.rs:1009-1016) with unified fields sourced from `ActivityDetailRow`, and compute the per-event rate:

```rust
        let unified = busytok_domain::cache_metrics::UnifiedCacheMetrics {
            prompt_input_total_tokens: event.prompt_input_total_tokens,
            prompt_input_non_cached_tokens: event.prompt_input_non_cached_tokens,
            cache_read_tokens: event.cache_read_tokens,
            cache_write_tokens: event.cache_creation_tokens,
        };
        let detail_rate = busytok_domain::cache_metrics::cache_hit_rate(unified);
        let token_breakdown = has_components.then(|| TokenBreakdownDto {
            prompt_input_total_tokens: (event.prompt_input_total_tokens > 0).then_some(event.prompt_input_total_tokens),
            prompt_input_non_cached_tokens: (event.prompt_input_non_cached_tokens > 0).then_some(event.prompt_input_non_cached_tokens),
            cache_read_tokens: (event.cache_read_tokens > 0).then_some(event.cache_read_tokens),
            cache_write_tokens: (event.cache_creation_tokens > 0).then_some(event.cache_creation_tokens),
            cache_hit_rate: detail_rate,
            input_tokens: (event.input_tokens > 0).then_some(event.input_tokens),
            output_tokens: (event.output_tokens > 0).then_some(event.output_tokens),
            cached_input_tokens: (event.cached_input_tokens > 0).then_some(event.cached_input_tokens),
            reasoning_tokens: (event.reasoning_tokens > 0).then_some(event.reasoning_tokens),
            total_tokens: event.total_tokens,
        });
```

Keep the `notes` vector (supervisor.rs:1018-1041) **as-is** — it remains the raw-audit technical-details area; the unified fields live on `TokenBreakdownDto` and do not replace the raw `cache_creation_tokens` / `cache_read_tokens` notes (Finding 3: do not collapse the two layers).

For the **model breakdown** (supervisor.rs:2411-2421), build from the extended `ModelTokenBreakdownRow` and compute the aggregate rate from sums:

```rust
                        let agg = busytok_domain::cache_metrics::UnifiedCacheMetrics {
                            prompt_input_total_tokens: token_breakdown_row.prompt_input_total_tokens,
                            prompt_input_non_cached_tokens: token_breakdown_row.prompt_input_non_cached_tokens,
                            cache_read_tokens: token_breakdown_row.cache_read_tokens,
                            cache_write_tokens: token_breakdown_row.cache_creation_tokens,
                        };
                        let token_breakdown = TokenBreakdownDto {
                            prompt_input_total_tokens: Some(token_breakdown_row.prompt_input_total_tokens).filter(|&v| v > 0),
                            prompt_input_non_cached_tokens: Some(token_breakdown_row.prompt_input_non_cached_tokens).filter(|&v| v > 0),
                            cache_read_tokens: Some(token_breakdown_row.cache_read_tokens).filter(|&v| v > 0),
                            cache_write_tokens: Some(token_breakdown_row.cache_creation_tokens).filter(|&v| v > 0),
                            cache_hit_rate: busytok_domain::cache_metrics::cache_hit_rate(agg),
                            input_tokens: Some(token_breakdown_row.input_tokens).filter(|&v| v > 0),
                            output_tokens: Some(token_breakdown_row.output_tokens).filter(|&v| v > 0),
                            cached_input_tokens: Some(token_breakdown_row.cached_input_tokens).filter(|&v| v > 0),
                            reasoning_tokens: Some(token_breakdown_row.reasoning_tokens).filter(|&v| v > 0),
                            total_tokens,
                        };
```

For the **session breakdown** (supervisor.rs:2588-2594), the components are currently all `None`. Sum them from the session's `activity_rows` (the `Vec<ActivityListRow>` already fetched at supervisor.rs:2485) and build the `TokenBreakdownDto` with the same aggregate pattern:

```rust
                        let sums = activity_rows.iter().fold(
                            (0i64, 0i64, 0i64, 0i64, 0i64), // total, non_cached, read, write, cached_input(raw)
                            |(t, nc, r, w, ci), row| (
                                t + row.prompt_input_total_tokens,
                                nc + row.prompt_input_non_cached_tokens,
                                r + row.cache_read_tokens,
                                w + row.cache_creation_tokens,
                                ci + row.cached_input_tokens,
                            ),
                        );
                        let agg = busytok_domain::cache_metrics::UnifiedCacheMetrics {
                            prompt_input_total_tokens: sums.0,
                            prompt_input_non_cached_tokens: sums.1,
                            cache_read_tokens: sums.2,
                            cache_write_tokens: sums.3,
                        };
                        let token_breakdown = TokenBreakdownDto {
                            prompt_input_total_tokens: Some(sums.0).filter(|&v| v > 0),
                            prompt_input_non_cached_tokens: Some(sums.1).filter(|&v| v > 0),
                            cache_read_tokens: Some(sums.2).filter(|&v| v > 0),
                            cache_write_tokens: Some(sums.3).filter(|&v| v > 0),
                            cache_hit_rate: busytok_domain::cache_metrics::cache_hit_rate(agg),
                            input_tokens: None,
                            output_tokens: None,
                            cached_input_tokens: Some(sums.4).filter(|&v| v > 0),
                            reasoning_tokens: None,
                            total_tokens,
                        };
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p busytok-runtime`
Expected: PASS. (The whole workspace should now compile; run `cargo test --workspace` to confirm no other crate regressed.)

- [ ] **Step 5: Commit**

```bash
git add crates/busytok-runtime/src/supervisor.rs
git commit -m "feat(runtime): unified cache_hit_rate + breakdowns across activity/session/model"
```

---

## Task 10: Frontend — drop client recompute, show unified breakdown

**Files:**
- Modify: `apps/gui/src/pages/ActivityPage.tsx:119-121,147` (delete recompute; use DTO `cache_hit_rate`), `:50-65` (unchanged list display already uses the DTO field)
- Modify: `apps/gui/src/pages/SessionsPage.tsx:115-137` — render new breakdown fields + aggregate `cache_hit_rate` from `token_breakdown.cache_hit_rate`.
- Modify: `apps/gui/src/pages/ModelsPage.tsx:99-121` — same as Sessions.
- `apps/gui/src/lib/formatters.ts:170` — unchanged (already `null → "--"`).
- Test: `apps/gui/src/pages/ActivityPage.test.tsx`, plus Sessions/Models page tests.

**Interfaces:**
- Consumes: regenerated `TokenBreakdownDto` (cache_hit_rate + unified fields), `ActivityListItemDto.cache_hit_rate`.
- Produces: no client-side `cached_input_tokens / input_tokens` arithmetic anywhere; detail drawers show Total/Non-cached Prompt Input, Cache Read, Cache Write, Cache Hit Rate.

- [ ] **Step 1: Write the failing test**

In `apps/gui/src/pages/ActivityPage.test.tsx`, add a test rendering a detail drawer whose `token_breakdown.cache_hit_rate = 0.99` and asserting the rendered Cache Hit Rate shows `99.00%` and that the component does NOT read `cached_input_tokens` for the rate. Mirror the existing detail-drawer test setup in that file (around line 395+).

```tsx
it("shows the DTO cache_hit_rate in the detail drawer without recomputing", () => {
  renderDetailDrawer({ token_breakdown: { /* … */ cache_hit_rate: 0.99, cached_input_tokens: 990 } });
  expect(screen.getByText(/99\.00%/)).toBeInTheDocument();
});
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `pnpm --filter @busytok/gui test -- ActivityPage`
Expected: FAIL — drawer still recomputes from `cached_input_tokens`.

- [ ] **Step 3: Write the implementation**

In `apps/gui/src/pages/ActivityPage.tsx`, delete the recompute block at lines 119-121:

```tsx
// DELETE:
const cacheHitRate = tk.input_tokens != null && tk.input_tokens > 0
  ? (tk.cached_input_tokens ?? 0) / tk.input_tokens
  : null;
```

and change the render at `:147` to read `tk.cache_hit_rate` (the DTO field) instead of the local `cacheHitRate`:

```tsx
<dd>{formatCacheHitRate(tk?.cache_hit_rate ?? null)}</dd>
```

Extend the detail drawer's breakdown grid to show the unified fields (Total Prompt Input, Non-cached Prompt Input, Cache Read, Cache Write, Cache Hit Rate) using the new `TokenBreakdownDto` fields as the **primary** breakdown. The raw `cached_input_tokens` stays on the DTO and may appear in a technical-details/debug area, but must NOT be used to compute any rate.

In `SessionsPage.tsx` (`:115-137`) and `ModelsPage.tsx` (`:99-121`), render the unified fields (`prompt_input_total_tokens`, `prompt_input_non_cached_tokens`, `cache_read_tokens`, `cache_write_tokens`) as the primary breakdown and add a Cache Hit Rate row sourced from `token_breakdown.cache_hit_rate` via `formatCacheHitRate`. Raw `cached_input_tokens` remains on the DTO for a debug area if those pages have one; it is not used for any rate.

- [ ] **Step 4: Run the test to verify it passes**

Run: `pnpm --filter @busytok/gui test` then `pnpm --filter @busytok/gui typecheck`
Expected: PASS, and typecheck clean (the rate is read from `cache_hit_rate`, never recomputed from `cached_input_tokens`).

- [ ] **Step 5: Commit**

```bash
git add apps/gui/src/pages/ActivityPage.tsx apps/gui/src/pages/SessionsPage.tsx apps/gui/src/pages/ModelsPage.tsx
git commit -m "feat(gui): consume unified cache metrics; drop client-side rate recompute"
```

---

## Task 11: Regression suite for the spec's five scenarios

**Files:**
- Test: extend `crates/busytok-domain/src/cache_metrics.rs` tests (scenarios 1-4) and `apps/gui/src/lib/formatters.test.ts` (scenario 5). Adapter-level coverage already added in Tasks 3-4.

- [ ] **Step 1: Write the failing tests (they should mostly pass after Tasks 1-10; add any gaps)**

Scenarios 1-4 are Rust; add to `cache_metrics.rs` tests if not already covered by Task 1:
1. Codex normal hit → rate in `[0,1]` (covered).
2. Anthropic-native → total includes cache, rate correct (covered).
3. Compatible small-input/large-cache → rate `<= 1.0`, no 3000%+ (covered).
4. Anomalous (native with cache > input) → `cache_hit_rate = None` (covered); the write-path diagnostic lifecycle (record on violation, clear on valid rewrite) is asserted in Task 6 (`cache_metric_diagnostic_lifecycle_records_and_recovers`).

Scenario 5 (frontend) — add to `apps/gui/src/lib/formatters.test.ts`:

```tsx
it("formats null as -- and never exceeds 100%", () => {
  expect(formatCacheHitRate(null)).toBe("--");
  expect(formatCacheHitRate(1.0)).toBe("100.00%");
  expect(formatCacheHitRate(0.99)).toBe("99.00%");
  expect(formatCacheHitRate(0.0)).toBe("0%");
});
```

(The formatter already clamps display via the `[0,1]` guarantee upstream; this test locks the contract.)

- [ ] **Step 2: Run all tests**

Run: `cargo test --workspace` then `pnpm -r test` then `pnpm --filter @busytok/gui typecheck`
Expected: PASS across the workspace and frontend.

- [ ] **Step 3: Verify acceptance criteria manually**

- Confirm no code path computes `cached_input_tokens / input_tokens` as a product rate: `grep -rn "cached_input_tokens.*input_tokens\|cache_hit_rate =" crates/ apps/gui/src | grep -v test` should show only the unified `cache_hit_rate(m)` call and the demoted raw field storage.
- Confirm `cache_hit_rate` can never exceed 1.0: the only producer is `cache_hit_rate()` (domain), guarded by the invariant + range check.

- [ ] **Step 4: Commit**

```bash
git add crates/busytok-domain/src/cache_metrics.rs apps/gui/src/lib/formatters.test.ts
git commit -m "test: cache-metrics regression suite for codex/anthropic/compatible/anomaly/display"
```

---

## Coverage Gate

The per-task tests are the functional gate; this section makes coverage an explicit, runnable acceptance action rather than an implied aspiration. Grounded in tooling that actually exists in the repo (verified: only the GUI has coverage tooling — `@vitest/coverage-v8` + `pnpm test:coverage`; no Rust coverage tooling is present).

**Frontend — real tooling, hard gate:**
- Run: `pnpm --filter @busytok/gui test:coverage`
- Required: ≥90% line coverage on the changed GUI files — `apps/gui/src/lib/formatters.ts`, `apps/gui/src/pages/ActivityPage.tsx`, `SessionsPage.tsx`, `ModelsPage.tsx`. A changed file below 90% blocks completion.

**Rust — functional gate now; % gate needs setup:**
- Functional gate (already required per task): every task's `cargo test -p <crate>` is green; final `cargo test --workspace` green.
- A hard Rust line-coverage gate (`cargo llvm-cov -p busytok-domain -p busytok-adapters -p busytok-store -p busytok-runtime --fail-under-lines 90`) requires installing `cargo-llvm-cov` + the `llvm-tools-preview` rustup component, which is **not** currently set up in this repo. Do not silently assume it. Choose one:
  1. **Default:** accept the green per-task `cargo test` suite as the Rust gate for this refactor; defer Rust %-coverage to a separate tooling task.
  2. **Opt-in:** add a Task 12 that installs `cargo-llvm-cov`, wires it into CI, and enforces `--fail-under-lines 90` on the four changed crates.
- Note: this plan itself never asserted a 90% figure; this section exists to make any coverage expectation concrete and runnable rather than implied.

---

## Self-Review

**1. Spec coverage:**
- §一 Problem definition (input_tokens overloaded) → resolved by `ProviderPayloadShape` (Tasks 1, 3, 4). Boundary reframed per Finding 4: unified metrics derived at every `NormalizedUsageEvent` construction site (Claude adapter + Codex delta builder) via one shared domain fn; provider knowledge there is shape + raw values only.
- §二 Two-layer model (raw audit + unified) → Tasks 1, 2, 5, 6 (raw kept; unified added).
- §三 Mapping rules (Codex / native / compatible) → `UnifiedCacheMetrics::from_raw` (Task 1) + adapter calls (Tasks 3, 4).
- §四 Deprecated logic (`cached_input_tokens / input_tokens`, >100% clamp, `cached <= input` assumption) → removed: Task 9 (supervisor rate), Task 10 (frontend recompute). No client clamp ever existed; server-side null-on-violation + centralized write-path diagnostic replaces it (Tasks 1, 6, 9). **`total_tokens` is untouched** (Finding 1) — audit semantics preserved.
- §五 Data model (NormalizedUsageEvent unified fields; cached_input_tokens demoted; read_models/DTO/protocol-types) → Tasks 2, 7, 8. `cached_input_tokens` kept as raw audit everywhere **including `TokenBreakdownDto`** (Finding 3); `total_tokens` audit semantics untouched (Finding 1). Pre-launch destructive dev-DB reset required (Finding 2; Task 5).
- §六 Display layer (Activity, Overview, Session, Model; null→--) → Tasks 9, 10. Project detail intentionally out of scope (decision); tooltip/ledger/export do not exist (confirmed by exploration).
- §七 Diagnostics + invariants → `invariant_holds` (Task 1) + **centralized write-path diagnostic emission** (Task 6) + null rate (Task 9). Adapters no longer assemble diagnostics (Finding 5).
- §八 Tests (5 scenarios) → Task 11 (scenarios 1-5) + Tasks 1, 3, 4 unit coverage.
- §九 Acceptance → Task 11 Step 3 grep verification + per-task passing tests.
- §十 Implementation order → task ordering matches (core → adapters → domain/store/DTO → runtime/API → frontend → tests).

**2. Placeholder scan:** Task test bodies reference real symbols (`UnifiedCacheMetrics`, `activity_item_from_read_row`, `read_activity_list`, `formatCacheHitRate`, `TokenBreakdownDto`). Where a helper/signature name could not be confirmed verbatim from exploration (`parse_claude_usage_line`, `build_codex_delta_events_for_test`, `Database::write_usage_event` arg order, `read_activity_list` arg list), the task says "mirror an existing test's call" with the exact assertion target — acceptable because the implementer reads the neighboring test for the plumbing. No "TODO/TBD/handle edge cases" anywhere.

**3. Type consistency:** Field names are uniform end-to-end: `prompt_input_total_tokens`, `prompt_input_non_cached_tokens`, `cache_read_tokens`, `cache_write_tokens` (DTO) ← `cache_creation_tokens` (raw/domain/DB). `cache_hit_rate: Option<f64>` on both `ActivityListItemDto` and `TokenBreakdownDto`. `provider_payload_shape` spelled identically in domain enum, domain field, DB column, and `ActivityDetailRow`. `cached_input_tokens` retained as raw audit **everywhere, including `TokenBreakdownDto`** (Finding 3 — not removed). `total_tokens` retains its existing audit semantics and is never recomputed from unified fields (Finding 1).
