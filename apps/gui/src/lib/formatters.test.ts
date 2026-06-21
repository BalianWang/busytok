import { describe, it, expect } from "vitest";
import {
  formatCost,
  formatDateTime,
  formatShortDate,
  formatRelativeTime,
  formatBytes,
  formatCompactNumber,
  formatCacheHitRate,
} from "./formatters";

describe("formatCost", () => {
  it("formats exact cost", () => {
    expect(formatCost(1.23, "exact")).toBe("$1.23");
  });

  it("formats partial cost same as exact", () => {
    expect(formatCost(5.0, "partial")).toBe("$5.00");
  });

  it("returns N/A when cost is null", () => {
    expect(formatCost(null, "exact")).toBe("N/A");
  });

  it("returns N/A when status is unavailable", () => {
    expect(formatCost(0, "unavailable")).toBe("N/A");
  });

  it("uses 2 decimals for >= $0.10", () => {
    expect(formatCost(0.15, "exact")).toBe("$0.15");
    expect(formatCost(1.0, "exact")).toBe("$1.00");
    expect(formatCost(100.0, "exact")).toBe("$100.00");
  });

  it("uses 3 decimals for >= $0.01", () => {
    expect(formatCost(0.012, "exact")).toBe("$0.012");
    expect(formatCost(0.099, "exact")).toBe("$0.099");
  });

  it("uses 4 decimals for >= $0.001", () => {
    expect(formatCost(0.0012, "exact")).toBe("$0.0012");
    expect(formatCost(0.0099, "exact")).toBe("$0.0099");
  });

  it("uses 5 decimals for >= $0.0001", () => {
    expect(formatCost(0.00012, "exact")).toBe("$0.00012");
  });

  it("uses 6 decimals for > $0 below $0.0001", () => {
    expect(formatCost(0.0000005438, "exact")).toBe("$0.000001");
    expect(formatCost(0.0000015, "exact")).toBe("$0.000002");
  });

  it("formats zero as $0.00", () => {
    expect(formatCost(0, "exact")).toBe("$0.00");
  });

  it("combines partial status with adaptive decimals", () => {
    expect(formatCost(0.012, "partial")).toBe("$0.012");
  });
});

describe("formatDateTime", () => {
  it("returns a non-empty string for a given timestamp", () => {
    const result = formatDateTime(1_700_000_000_000);
    expect(result).toBeTruthy();
    expect(typeof result).toBe("string");
  });

  it("includes date parts in the output", () => {
    // 2023-11-14T22:13:20.000Z
    const result = formatDateTime(1_700_000_000_000);
    expect(result).toMatch(/\d{4}/);
  });
});

describe("formatShortDate", () => {
  it("formats UTC timestamp as YYYY/MM/DD", () => {
    // 2023-11-14T22:13:20.000Z
    expect(formatShortDate(1_700_000_000_000, "UTC")).toBe("2023/11/14");
  });

  it("uses slash separators, never dashes", () => {
    const result = formatShortDate(1_700_000_000_000, "UTC");
    expect(result).toMatch(/^\d{4}\/\d{2}\/\d{2}$/);
    expect(result).not.toContain("-");
  });

  it("zero-pads single-digit month and day", () => {
    // 2023-01-05T00:00:00.000Z
    const jan5 = Date.UTC(2023, 0, 5);
    expect(formatShortDate(jan5, "UTC")).toBe("2023/01/05");
  });

  it("respects the timezone argument", () => {
    // 2023-11-14T22:13:20.000Z is 2023-11-15 in Asia/Shanghai (+08:00)
    expect(formatShortDate(1_700_000_000_000, "Asia/Shanghai")).toBe("2023/11/15");
  });

  it("produces locale-independent canonical output (no 年月日 / no MM/DD/YYYY flip)", () => {
    // No matter the system locale, the result must always be YYYY/MM/DD.
    const result = formatShortDate(1_700_000_000_000, "UTC");
    expect(result).toBe("2023/11/14");
    expect(result).not.toMatch(/年|月|日/);
  });
});

describe("formatRelativeTime", () => {
  it('returns "just now" for very recent times', () => {
    expect(formatRelativeTime(Date.now())).toBe("just now");
  });

  it('returns "just now" for future timestamps', () => {
    expect(formatRelativeTime(Date.now() + 10_000)).toBe("just now");
  });

  it("returns minutes ago", () => {
    const fiveMinAgo = Date.now() - 5 * 60_000;
    expect(formatRelativeTime(fiveMinAgo)).toBe("5m ago");
  });

  it("returns hours ago", () => {
    const threeHoursAgo = Date.now() - 3 * 3_600_000;
    expect(formatRelativeTime(threeHoursAgo)).toBe("3h ago");
  });

  it("returns days ago", () => {
    const twoDaysAgo = Date.now() - 2 * 86_400_000;
    expect(formatRelativeTime(twoDaysAgo)).toBe("2d ago");
  });

  it("returns weeks ago", () => {
    const threeWeeksAgo = Date.now() - 3 * 604_800_000;
    expect(formatRelativeTime(threeWeeksAgo)).toBe("3w ago");
  });
});

describe("formatBytes", () => {
  it("formats 0 bytes", () => {
    expect(formatBytes(0)).toBe("0 B");
  });

  it("formats bytes", () => {
    expect(formatBytes(500)).toBe("500.0 B");
  });

  it("formats kilobytes", () => {
    expect(formatBytes(1024)).toBe("1.0 KB");
    expect(formatBytes(1536)).toBe("1.5 KB");
  });

  it("formats megabytes", () => {
    expect(formatBytes(1_048_576)).toBe("1.0 MB");
  });

  it("formats gigabytes", () => {
    expect(formatBytes(1_073_741_824)).toBe("1.0 GB");
  });
});

describe("formatCompactNumber", () => {
  it("formats values under 1000 as locale integers", () => {
    expect(formatCompactNumber(0)).toBe("0");
    expect(formatCompactNumber(500)).toBe("500");
    expect(formatCompactNumber(999)).toBe("999");
  });

  it("formats thousands with uppercase K", () => {
    expect(formatCompactNumber(1_000)).toBe("1K");
    expect(formatCompactNumber(1_500)).toBe("1.5K");
    expect(formatCompactNumber(24_000)).toBe("24K");
  });

  it("formats millions with uppercase M", () => {
    expect(formatCompactNumber(1_000_000)).toBe("1M");
    expect(formatCompactNumber(3_400_000)).toBe("3.4M");
    expect(formatCompactNumber(24_000_000)).toBe("24M");
  });

  it("formats billions with uppercase B", () => {
    expect(formatCompactNumber(1_000_000_000)).toBe("1B");
    expect(formatCompactNumber(2_500_000_000)).toBe("2.5B");
  });

  it("strips trailing .0 for whole numbers", () => {
    expect(formatCompactNumber(2_000)).toBe("2K");
    expect(formatCompactNumber(3_000_000)).toBe("3M");
    expect(formatCompactNumber(10_000_000_000)).toBe("10B");
  });

  it("carries over to next unit when rounding reaches 1000", () => {
    expect(formatCompactNumber(999_999)).toBe("1M");
    expect(formatCompactNumber(999_999_999)).toBe("1B");
    expect(formatCompactNumber(999_000_000)).toBe("999M");
  });

  it("handles negative values", () => {
    expect(formatCompactNumber(-500)).toBe("-500");
    expect(formatCompactNumber(-1_500)).toBe("-1.5K");
    expect(formatCompactNumber(-3_400_000)).toBe("-3.4M");
  });

  it("accepts bigint values", () => {
    expect(formatCompactNumber(1_000n)).toBe("1K");
    expect(formatCompactNumber(5_000_000n)).toBe("5M");
    expect(formatCompactNumber(999_999n)).toBe("1M");
  });
});

describe("formatCacheHitRate", () => {
  it("returns -- for null", () => {
    expect(formatCacheHitRate(null)).toBe("--");
  });

  it("returns -- for NaN", () => {
    expect(formatCacheHitRate(NaN)).toBe("--");
  });

  it("returns -- for Infinity", () => {
    expect(formatCacheHitRate(Infinity)).toBe("--");
  });

  it("returns -- for -Infinity", () => {
    expect(formatCacheHitRate(-Infinity)).toBe("--");
  });

  it("returns 0% for 0", () => {
    expect(formatCacheHitRate(0)).toBe("0%");
  });

  it("returns 0% for negative values", () => {
    expect(formatCacheHitRate(-0.1)).toBe("0%");
  });

  it("formats typical value with two decimals", () => {
    expect(formatCacheHitRate(0.3)).toBe("30.00%");
    expect(formatCacheHitRate(0.4527)).toBe("45.27%");
  });

  it("truncates rather than rounds", () => {
    expect(formatCacheHitRate(0.425)).toBe("42.50%");
    expect(formatCacheHitRate(0.424)).toBe("42.40%");
  });

  it("formats high-precision values without collapsing to 100%", () => {
    expect(formatCacheHitRate(0.99998)).toBe("99.99%");
    expect(formatCacheHitRate(0.999)).toBe("99.90%");
    expect(formatCacheHitRate(0.995)).toBe("99.50%");
  });

  it("reaches 100.00% only for exactly 1.0", () => {
    expect(formatCacheHitRate(1.0)).toBe("100.00%");
  });

  it("shows sub-percent values precisely", () => {
    expect(formatCacheHitRate(0.003)).toBe("0.30%");
    expect(formatCacheHitRate(0.001)).toBe("0.10%");
  });
});
