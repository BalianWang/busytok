import { describe, expect, it } from "vitest";
import {
  deriveProviderName,
  deriveUniqueProviderName,
  errorMessage,
  parseTags,
  validateBaseUrl,
  KIND_LABELS,
  KIND_OPTIONS,
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
    expect(validateBaseUrl("")).toBe("Base URL is required");
  });
  it("returns error for whitespace-only input", () => {
    expect(validateBaseUrl("   ")).toBe("Base URL is required");
  });
  it("returns error for missing protocol", () => {
    expect(validateBaseUrl("api.deepseek.com")).toBe("Enter a complete URL starting with http:// or https://");
  });
  it("returns error for ftp protocol", () => {
    expect(validateBaseUrl("ftp://api.deepseek.com")).toBe("Enter a complete URL starting with http:// or https://");
  });
  it("returns error for malformed URL", () => {
    expect(validateBaseUrl("https://")).toBe("Invalid URL format");
  });
});

describe("errorMessage", () => {
  it("extracts message from Error instance", () => {
    expect(errorMessage(new Error("network failure"), "fallback")).toBe("network failure");
  });
  it("returns fallback for Error with empty message", () => {
    expect(errorMessage(new Error(""), "fallback")).toBe("fallback");
  });
  it("returns the string when error is a non-empty string", () => {
    expect(errorMessage("something went wrong", "fallback")).toBe("something went wrong");
  });
  it("returns fallback for empty string", () => {
    expect(errorMessage("", "fallback")).toBe("fallback");
  });
  it("returns fallback for non-Error, non-string values", () => {
    expect(errorMessage({}, "fallback")).toBe("fallback");
    expect(errorMessage(null, "fallback")).toBe("fallback");
    expect(errorMessage(undefined, "fallback")).toBe("fallback");
    expect(errorMessage(42, "fallback")).toBe("fallback");
  });
});

describe("KIND_LABELS", () => {
  it("maps known provider kinds to display labels", () => {
    expect(KIND_LABELS["openai_compatible"]).toBe("OpenAI-compatible");
    expect(KIND_LABELS["anthropic_compatible"]).toBe("Anthropic-compatible");
  });
  it("returns undefined for unknown kinds (callers fall back to raw value)", () => {
    expect(KIND_LABELS["custom_kind"]).toBeUndefined();
  });
});

describe("KIND_OPTIONS", () => {
  it("contains all keys from KIND_LABELS", () => {
    expect(KIND_OPTIONS).toEqual(Object.keys(KIND_LABELS));
    expect(KIND_OPTIONS).toContain("openai_compatible");
    expect(KIND_OPTIONS).toContain("anthropic_compatible");
  });
});
