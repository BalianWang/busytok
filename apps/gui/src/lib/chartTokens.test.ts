import { describe, expect, it } from "vitest";
import { chartTokens } from "./chartTokens";

describe("chartTokens", () => {
  it("returns indigo-first data roles instead of semantic success colors", () => {
    expect(chartTokens.linePrimary).toBe("var(--color-data-primary)");
    expect(chartTokens.livePrimary).toBe("var(--color-data-live-primary)");
    expect(chartTokens.lineSecondary).toBe("var(--color-data-secondary)");
    expect(chartTokens.lineTertiary).toBe("var(--color-data-tertiary)");
  });

  it("exposes a dedicated transient attention role", () => {
    expect(chartTokens.lineAttention).toBe("var(--color-data-attention)");
    expect(chartTokens.lineNeutral).toBe("var(--color-data-neutral)");
  });

  it("exposes soft variants for fills and gradients", () => {
    expect(chartTokens.linePrimarySoft).toBe("var(--color-data-primary-soft)");
    expect(chartTokens.livePrimarySoft).toBe("var(--color-data-live-primary-soft)");
    expect(chartTokens.lineSecondarySoft).toBe("var(--color-data-secondary-soft)");
    expect(chartTokens.lineTertiarySoft).toBe("var(--color-data-tertiary-soft)");
  });

  it("exposes neutral surface tokens for axis/grid/tooltip text", () => {
    expect(chartTokens.textMuted).toBe("var(--color-text-muted)");
    expect(chartTokens.borderSubtle).toBe("var(--color-border-subtle)");
  });

  it("is frozen so stray assignments cannot corrupt every chart", () => {
    expect(Object.isFrozen(chartTokens)).toBe(true);
    // ESM is strict mode, so mutating a frozen property throws TypeError.
    expect(() => {
      (chartTokens as { linePrimary?: string }).linePrimary = "var(--evil)";
    }).toThrow(TypeError);
    expect(chartTokens.linePrimary).toBe("var(--color-data-primary)");
  });
});
