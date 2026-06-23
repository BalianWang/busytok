import { memo } from "react";
import type { TokenBreakdownDto } from "@busytok/protocol-types";
import { formatCacheHitRate } from "../lib/formatters";

interface TokenBreakdownProps {
  tk: TokenBreakdownDto;
}

/**
 * Unified cache-metric token breakdown + a collapsible Raw Audit section.
 *
 * The primary `<dl>` shows the unified product metrics (cross-provider,
 * invariant-guarded `cache_hit_rate`). The Raw Audit `<details>` exposes the
 * provider's LITERAL token fields — `input_tokens` and `cached_input_tokens` —
 * which reveal how the provider reported usage (e.g. whether `input_tokens`
 * already includes the cache portion), for debugging provider-semantic
 * classification. This is the two-layer model's raw-audit surface: the raw
 * fields are kept on the DTO but never feed a product metric.
 */
export const TokenBreakdown = memo(function TokenBreakdown({ tk }: TokenBreakdownProps) {
  return (
    <section className="token-breakdown">
      <h3>Token Breakdown</h3>
      <dl>
        <dt>Total</dt>
        <dd>{tk.total_tokens.toLocaleString()}</dd>
        {tk.prompt_input_total_tokens != null && (
          <>
            <dt>Prompt Input (Total)</dt>
            <dd>{tk.prompt_input_total_tokens.toLocaleString()}</dd>
          </>
        )}
        {tk.prompt_input_non_cached_tokens != null && (
          <>
            <dt>Prompt Input (Non-cached)</dt>
            <dd>{tk.prompt_input_non_cached_tokens.toLocaleString()}</dd>
          </>
        )}
        {tk.cache_read_tokens != null && (
          <>
            <dt>Cache Read</dt>
            <dd>{tk.cache_read_tokens.toLocaleString()}</dd>
          </>
        )}
        {tk.cache_write_tokens != null && (
          <>
            <dt>Cache Write</dt>
            <dd>{tk.cache_write_tokens.toLocaleString()}</dd>
          </>
        )}
        <dt>Cache Hit Rate</dt>
        <dd>{formatCacheHitRate(tk.cache_hit_rate)}</dd>
        {tk.output_tokens != null && (
          <>
            <dt>Output</dt>
            <dd>{tk.output_tokens.toLocaleString()}</dd>
          </>
        )}
        {tk.reasoning_tokens != null && (
          <>
            <dt>Reasoning</dt>
            <dd>{tk.reasoning_tokens.toLocaleString()}</dd>
          </>
        )}
      </dl>

      {/* Raw Audit: the provider's literal token fields, for debugging shape
          classification. Collapsed by default (debug surface, not a product
          metric). */}
      <details className="token-breakdown__raw-audit">
        <summary>Raw Audit</summary>
        <dl>
          <dt>Input Tokens (raw)</dt>
          <dd>{(tk.input_tokens ?? 0).toLocaleString()}</dd>
          <dt>Cached Input Tokens (raw)</dt>
          <dd>{(tk.cached_input_tokens ?? 0).toLocaleString()}</dd>
        </dl>
      </details>
    </section>
  );
});
