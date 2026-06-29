//! Formatting utilities for displaying Busytok data in the GUI.

/**
 * Adaptive decimal places for cost display.
 *
 * - >= $0.10  → 2 decimals  ($1.23, $0.15)
 * - >= $0.01  → 3 decimals  ($0.012)
 * - >= $0.001 → 4 decimals  ($0.0012)
 * - >= $0.0001 → 5 decimals ($0.00012)
 * - > $0      → 6 decimals  ($0.000001)
 * - = $0      → "$0.00"
 */
export function formatCostValue(cost_usd: number): string {
  if (cost_usd === 0) return "$0.00";
  const abs = Math.abs(cost_usd);
  const decimals =
    abs >= 0.1 ? 2 :
    abs >= 0.01 ? 3 :
    abs >= 0.001 ? 4 :
    abs >= 0.0001 ? 5 :
    6;
  return `$${cost_usd.toFixed(decimals)}`;
}

/**
 * Format a cost value with its status.
 * Returns "$X.XX" for exact/partial, "N/A" for unavailable.
 * Uses adaptive decimals for small amounts so sub-cent costs are visible.
 */
export function formatCost(
  cost_usd: number | null,
  cost_status: string,
): string {
  if (cost_usd === null || cost_status === "unavailable") return "N/A";
  const formatted = formatCostValue(cost_usd);
  return formatted;
}

/**
 * Format a millisecond timestamp as a locale-independent short date (YYYY/MM/DD).
 *
 * Uses the en-CA locale (which formats as YYYY-MM-DD) and rewrites dashes to
 * slashes. This avoids the locale-dependent output of `DateTimeFormat` (e.g.,
 * "11/14/2023" in en-US, "14/11/2023" in en-GB, "2023年11月14日" in zh-CN)
 * for surfaces that want a single canonical display format.
 *
 * If timezone is provided, formats in that timezone; otherwise uses the
 * system local timezone (so the date matches the user's calendar).
 */
export function formatShortDate(ms: number, timezone?: string): string {
  const date = new Date(ms);
  const options: Intl.DateTimeFormatOptions = {
    year: "numeric",
    month: "2-digit",
    day: "2-digit",
  };
  if (timezone) {
    options.timeZone = timezone;
  }
  return new Intl.DateTimeFormat("en-CA", options).format(date).replace(/-/g, "/");
}

/**
 * Format a millisecond timestamp into a locale date/time string.
 * If timezone is provided, uses that timezone.
 */
export function formatDateTime(ms: number, timezone?: string): string {
  const date = new Date(ms);
  const options: Intl.DateTimeFormatOptions = {
    year: "numeric",
    month: "short",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  };
  if (timezone) {
    options.timeZone = timezone;
  }
  return date.toLocaleString(undefined, options);
}

/**
 * Format a millisecond timestamp as a relative time string.
 * Examples: "2h ago", "3d ago", "just now"
 */
export function formatRelativeTime(ms: number): string {
  const now = Date.now();
  const diff = now - ms;

  if (diff < 60_000) return "just now";
  if (diff < 3_600_000) {
    const minutes = Math.floor(diff / 60_000);
    return `${minutes}m ago`;
  }
  if (diff < 86_400_000) {
    const hours = Math.floor(diff / 3_600_000);
    return `${hours}h ago`;
  }
  if (diff < 604_800_000) {
    const days = Math.floor(diff / 86_400_000);
    return `${days}d ago`;
  }
  if (diff < 2_592_000_000) {
    const weeks = Math.floor(diff / 604_800_000);
    return `${weeks}w ago`;
  }
  const months = Math.floor(diff / 2_592_000_000);
  return `${months}mo ago`;
}

/**
 * Format a number with an adaptive SI-style prefix (uppercase K, M, B).
 *
 * Values under 1,000 are displayed as locale-formatted integers.  Larger
 * values are scaled to the nearest prefix with one decimal place.  When
 * rounding would push a scaled value to or above 1,000 the formatter
 * promotes to the next unit so "999.9K" becomes "1M".
 *
 * Trailing ".0" is stripped so whole numbers read "2M" rather than "2.0M".
 *
 * Examples: "500", "1.5K", "24M", "3B", "1,234" (< 1,000 uses locale)
 */
export function formatCompactNumber(n: number | bigint): string {
  const num = typeof n === "bigint" ? Number(n) : n;

  // B (billions)
  if (Math.abs(num) >= 1_000_000_000) {
    return _fmtAtScale(num, 1_000_000_000, "B", 1_000_000_000_000, "T");
  }
  // M (millions)
  if (Math.abs(num) >= 1_000_000) {
    return _fmtAtScale(num, 1_000_000, "M", 1_000_000_000, "B");
  }
  // K (thousands)
  if (Math.abs(num) >= 1_000) {
    return _fmtAtScale(num, 1_000, "K", 1_000_000, "M");
  }

  return Math.round(num).toLocaleString();
}

/** Format `n` at `divisor` scale, carrying over to `nextDivisor`/`nextSuffix`
 *  when the rounded result reaches or exceeds 1,000. */
function _fmtAtScale(
  n: number,
  divisor: number,
  suffix: string,
  nextDivisor: number,
  nextSuffix: string,
): string {
  const scaled = n / divisor;
  const rounded = parseFloat(scaled.toFixed(1));
  if (rounded >= 1_000) {
    return _stripZero((n / nextDivisor).toFixed(1)) + nextSuffix;
  }
  return _stripZero(scaled.toFixed(1)) + suffix;
}

function _stripZero(s: string): string {
  return s.replace(/\.0$/, "");
}

/**
 * Format a cache hit rate (0.0–1.0) as a two-decimal percentage string.
 * Returns "--" when null/NaN/Infinity, "0%" for ≤ 0.
 * Truncates (floor) rather than rounds — cache hit rate should never be
 * overstated. DeepSeek users routinely see 99.9%+ rates where integer
 * display would collapse everything to "100%".
 */
export function formatCacheHitRate(rate: number | null): string {
  if (rate === null || !Number.isFinite(rate)) return "--";
  if (rate <= 0.0) return "0%";
  const pct = Math.floor(rate * 10000) / 100;
  return `${pct.toFixed(2)}%`;
}

/**
 * Format a byte count into a human-readable string.
 * Examples: "1.2 MB", "3.4 GB"
 */
export function formatBytes(bytes: number): string {
  if (bytes === 0) return "0 B";
  const units = ["B", "KB", "MB", "GB", "TB"];
  const i = Math.min(
    Math.floor(Math.log(bytes) / Math.log(1024)),
    units.length - 1,
  );
  const value = bytes / Math.pow(1024, i);
  return `${value.toFixed(1)} ${units[i]}`;
}
