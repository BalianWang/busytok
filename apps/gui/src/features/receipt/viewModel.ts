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
  others: boolean;
}

export interface ReceiptViewModel {
  dateLabel: string;
  timezone: string;
  hero: { totalTokens: string; totalTokensRaw: number };
  secondary: {
    split: string; // "in 2.1M · out 0.9M · cache 1.8M"
    cost: string; // "est. $47.21" | "est. —"
    cacheHitRate: string;
  };
  items: ReceiptItem[];
  total: { tokens: string; cost: string };
  peakHour: string | null;
  serial: string; // "#0626-A3F2"
}

function formatReceiptCost(costUsd: number | null, status: CostStatusDto): string {
  if (status === "unavailable" || costUsd === null) return "—";
  // formatCostValue ALREADY returns a "$X.XX" string — do NOT re-prefix "$"
  // (double-"$" bug: would render "$$24.10").
  const value = formatCostValue(costUsd);
  return status === "partial" ? `≈${value}` : value;
}

function worstStatus(rows: ReceiptModelSliceDto[]): CostStatusDto {
  if (rows.length === 0) return "unavailable";
  if (rows.every((r) => r.cost_status === "exact")) return "exact";
  if (rows.every((r) => r.cost_status === "unavailable")) return "unavailable";
  return "partial"; // mixed exact + partial/unavailable
}

function toItem(m: ReceiptModelSliceDto): ReceiptItem {
  return {
    name: m.name,
    tokens: formatCompactNumber(m.tokens),
    cost: formatReceiptCost(m.cost_usd, m.cost_status),
    others: false,
  };
}

function receiptSerial(date: string): string {
  // Deterministic, date-derived pseudo-serial for receipt authenticity.
  const digits = date.replace(/-/g, "").slice(4); // MMDD
  const hash = (date + "busytok")
    .split("")
    .reduce((acc, c) => (acc * 31 + c.charCodeAt(0)) >>> 0, 7);
  const suffix = hash.toString(16).toUpperCase().slice(0, 4).padStart(4, "0");
  return `#${digits}-${suffix}`;
}

export function toReceiptViewModel(dto: ReceiptDailyDto): ReceiptViewModel {
  const m = dto.metrics;
  const ranked = [...dto.top_models].sort((a, b) => b.tokens - a.tokens);
  const top = ranked.slice(0, TOP_N);
  const rest = ranked.slice(TOP_N);

  const items: ReceiptItem[] = top.map(toItem);
  if (rest.length > 0) {
    const othersTokens = rest.reduce((s, r) => s + r.tokens, 0);
    const othersCostUsd = rest.reduce<number>((s, r) => s + (r.cost_usd ?? 0), 0);
    items.push({
      name: `OTHERS (${rest.length})`,
      tokens: formatCompactNumber(othersTokens),
      cost: formatReceiptCost(othersCostUsd, worstStatus(rest)),
      others: true,
    });
  }

  const estCost = `est. ${formatReceiptCost(m.cost_usd, m.cost_status)}`;

  return {
    dateLabel: dto.date_label,
    timezone: dto.timezone,
    hero: {
      totalTokens: formatCompactNumber(m.total_tokens),
      totalTokensRaw: m.total_tokens,
    },
    secondary: {
      split: `in ${formatCompactNumber(m.input_tokens)} · out ${formatCompactNumber(
        m.output_tokens,
      )} · cache ${formatCompactNumber(m.cache_read_tokens)}`,
      cost: estCost,
      cacheHitRate: formatCacheHitRate(m.cache_hit_rate),
    },
    items,
    total: {
      tokens: formatCompactNumber(m.total_tokens),
      cost: formatReceiptCost(m.cost_usd, m.cost_status),
    },
    peakHour: m.peak_hour?.label ?? null,
    serial: receiptSerial(dto.date),
  };
}
