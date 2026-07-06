import { describe, expect, it } from "vitest";
import {
  deriveProviderName,
  deriveUniqueProviderName,
  parseTags,
  validateBaseUrl,
} from "./providerFormUtils";

describe("parseTags", () => {
  it("returns empty array for empty string", () => {
    expect(parseTags("")).toEqual([]);
  });
  it("trims and splits on comma", () => {
    expect(parseTags("cheap, fast , reasoning")).toEqual(["cheap", "fast", "reasoning"]);
  });
  it("drops empty entries", () => {
    expect(parseTags("cheap,,fast,")).toEqual(["cheap", "fast"]);
  });
  it("handles whitespace-only entries", () => {
    expect(parseTags("  ,  ")).toEqual([]);
  });
});

describe("deriveProviderName", () => {
  it("derives domain_kind from a typical URL", () => {
    expect(deriveProviderName("https://api.deepseek.com/v1", "openai_compatible"))
      .toBe("deepseek_openai");
  });
  it("strips _compatible suffix from kind", () => {
    expect(deriveProviderName("https://api.anthropic.com", "anthropic_compatible"))
      .toBe("anthropic_anthropic");
  });
  it("falls back to full host for single-part hostnames", () => {
    expect(deriveProviderName("https://localhost:8080/v1", "openai_compatible"))
      .toBe("localhost_openai");
  });
  it("handles URL with port", () => {
    expect(deriveProviderName("http://host:3000", "openai_compatible"))
      .toBe("host_openai");
  });
});

describe("deriveUniqueProviderName", () => {
  it("returns base name when no collision", () => {
    const existing = new Set<string>(["other_openai"]);
    expect(deriveUniqueProviderName("https://api.deepseek.com", "openai_compatible", existing))
      .toBe("deepseek_openai");
  });
  it("appends _2 on first collision", () => {
    const existing = new Set<string>(["deepseek_openai"]);
    expect(deriveUniqueProviderName("https://api.deepseek.com", "openai_compatible", existing))
      .toBe("deepseek_openai_2");
  });
  it("increments suffix until unique", () => {
    const existing = new Set<string>(["deepseek_openai", "deepseek_openai_2", "deepseek_openai_3"]);
    expect(deriveUniqueProviderName("https://api.deepseek.com", "openai_compatible", existing))
      .toBe("deepseek_openai_4");
  });
});

describe("validateBaseUrl", () => {
  it("returns null for valid https URL", () => {
    expect(validateBaseUrl("https://api.deepseek.com/v1")).toBeNull();
  });
  it("returns null for valid http URL", () => {
    expect(validateBaseUrl("http://localhost:8080")).toBeNull();
  });
  it("returns error for empty input", () => {
    expect(validateBaseUrl("")).toBe("Base URL 不能为空");
  });
  it("returns error for whitespace-only input", () => {
    expect(validateBaseUrl("   ")).toBe("Base URL 不能为空");
  });
  it("returns error for missing protocol", () => {
    expect(validateBaseUrl("api.deepseek.com")).toBe("请输入完整的 URL（以 http:// 或 https:// 开头）");
  });
  it("returns error for ftp protocol", () => {
    expect(validateBaseUrl("ftp://api.deepseek.com")).toBe("请输入完整的 URL（以 http:// 或 https:// 开头）");
  });
  it("returns error for malformed URL", () => {
    expect(validateBaseUrl("https://")).toBe("URL 格式不正确");
  });
});
