import type {
  CostStatusDto,
  ReceiptDailyDto,
  ReceiptModelSliceDto,
} from "@busytok/protocol-types";
import {
  formatCacheHitRate,
  formatCompactNumber,
  formatCostValue,
} from "../../lib/formatters";

const TOP_N = 5;

export interface ReceiptItem {
  name: string;
  tokens: string;
  cost: string; // "$24.10" | "≈$24.10" | "—"
}

export interface ReceiptViewModel {
  dateLabel: string;
  /** Raw total token count, used for empty-state detection only. */
  totalTokensRaw: number;
  /** One-line compact summary of the input/output/cache split. */
  summary: string;
  /** Cache hit rate as "46.15%" / "--". */
  cacheHitRate: string;
  /** Receipt issuance time (HH:MM local) derived from brand.generated_at_ms. */
  generatedAtLabel: string;
  /** Top-N model rows (truncated to TOP_N). `truncated` indicates overflow. */
  items: ReceiptItem[];
  /** True when top_models had more than TOP_N entries (overflow indicator). */
  truncated: boolean;
  total: { tokens: string; cost: string };
}

function formatReceiptCost(costUsd: number | null, status: CostStatusDto): string {
  if (status === "unavailable" || costUsd === null) return "—";
  // formatCostValue ALREADY returns a "$X.XX" string — do NOT re-prefix "$"
  // (double-"$" bug: would render "$$24.10").
  const value = formatCostValue(costUsd);
  return status === "partial" ? `≈${value}` : value;
}

function toItem(m: ReceiptModelSliceDto): ReceiptItem {
  return {
    name: m.name,
    tokens: formatCompactNumber(m.tokens),
    cost: formatReceiptCost(m.cost_usd, m.cost_status),
  };
}

function formatGeneratedTime(ms: number): string {
  // brand.generated_at_ms is the server-side issuance timestamp. Render as
  // HH:MM in the viewer's local time — matches a real receipt's "ISSUED 22:34".
  if (!ms) return "—";
  const d = new Date(ms);
  const hh = String(d.getHours()).padStart(2, "0");
  const mm = String(d.getMinutes()).padStart(2, "0");
  return `${hh}:${mm}`;
}

export function toReceiptViewModel(dto: ReceiptDailyDto): ReceiptViewModel {
  const m = dto.metrics;
  const ranked = [...dto.top_models].sort((a, b) => b.tokens - a.tokens);
  const top = ranked.slice(0, TOP_N);

  return {
    dateLabel: dto.date_label,
    totalTokensRaw: m.total_tokens,
    summary: `in ${formatCompactNumber(m.input_tokens)} · out ${formatCompactNumber(
      m.output_tokens,
    )} · cache ${formatCompactNumber(m.cache_read_tokens)}`,
    cacheHitRate: formatCacheHitRate(m.cache_hit_rate),
    generatedAtLabel: formatGeneratedTime(dto.brand?.generated_at_ms ?? 0),
    items: top.map(toItem),
    // Truncated when more than TOP_N models — TOTAL still reflects the full
    // aggregate; items are just visually capped to keep the 3:4 layout stable.
    truncated: ranked.length > TOP_N,
    total: {
      tokens: formatCompactNumber(m.total_tokens),
      // Partial status is carried by the ≈ marker; no "est." prefix.
      cost: formatReceiptCost(m.cost_usd, m.cost_status),
    },
  };
}
