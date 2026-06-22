import { describe, expect, it } from "vitest";
import {
  parsePromptTags,
  promptDisplayHeadline,
  promptDisplayLabel,
  promptDisplayTitle,
  promptLastUsedLabel,
  promptUpdatedLabel,
  promptUseCountLabel,
} from "./promptPaletteText";

describe("promptPaletteText", () => {
  it("deduplicates tags case-insensitively while preserving first display casing", () => {
    expect(parsePromptTags("Review, review, Tests")).toEqual(["Review", "Tests"]);
  });

  it("returns alias-prefixed label when alias is present", () => {
    expect(promptDisplayLabel("sum", "  Summarize this\nthread  ")).toBe("sum: Summarize this thread");
  });

  it("returns content-only label when alias is null", () => {
    expect(promptDisplayLabel(null, "  Draft notes  ")).toBe("Draft notes");
  });

  describe("metadata labels", () => {
    it("promptUseCountLabel pluralizes", () => {
      expect(promptUseCountLabel(0)).toBe("0 uses");
      expect(promptUseCountLabel(1)).toBe("1 use");
      expect(promptUseCountLabel(5)).toBe("5 uses");
    });

    it("promptLastUsedLabel handles null and recent timestamp", () => {
      expect(promptLastUsedLabel(null)).toBe("Not used yet");
      expect(promptLastUsedLabel(Date.now() - 30_000)).toBe("Last used just now");
    });

    it("promptUpdatedLabel emits slash-format date, no locale-dependent 年月日", () => {
      // 2023-01-05T00:00:00.000Z
      const result = promptUpdatedLabel(Date.UTC(2023, 0, 5));
      expect(result).toMatch(/^Updated \d{4}\/\d{2}\/\d{2}$/);
      expect(result).not.toMatch(/年|月|日/);
    });
  });

  describe("promptDisplayTitle", () => {
    it("returns alias when present", () => {
      expect(promptDisplayTitle("tests", "anything")).toBe("tests");
    });

    it("returns compacted content when alias is null", () => {
      expect(promptDisplayTitle(null, "  Summarize\nthis  ")).toBe("Summarize this");
    });

    it("handles leading blank lines", () => {
      expect(promptDisplayTitle(null, "\n\nActual content here")).toBe("Actual content here");
    });

    it("handles indented-only first line", () => {
      expect(promptDisplayTitle(null, "   \n  Real text")).toBe("Real text");
    });

    it("truncates long content with ellipsis", () => {
      const result = promptDisplayTitle(null, "a".repeat(100));
      expect(result.length).toBe(80);
      expect(result.endsWith("…")).toBe(true);
    });
  });

  describe("promptDisplayHeadline", () => {
    it("returns alias: content when alias is present", () => {
      expect(promptDisplayHeadline("tests", "Create focused tests")).toBe("tests: Create focused tests");
    });

    it("returns compacted content when alias is null", () => {
      expect(promptDisplayHeadline(null, "  Summarize\nthis  ")).toBe("Summarize this");
    });

    it("truncates long alias+content at max length", () => {
      const result = promptDisplayHeadline("alias", "x".repeat(200));
      expect(result.length).toBe(121); // 120 chars + "…"
      expect(result.endsWith("…")).toBe(true);
      expect(result.startsWith("alias: ")).toBe(true);
    });

    it("truncates long content without alias at max length", () => {
      const result = promptDisplayHeadline(null, "x".repeat(200));
      expect(result.length).toBe(121);
      expect(result.endsWith("…")).toBe(true);
    });
  });
});
