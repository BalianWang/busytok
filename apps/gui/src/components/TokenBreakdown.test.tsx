import { render, screen, cleanup } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import type { TokenBreakdownDto } from "@busytok/protocol-types";
import { TokenBreakdown } from "./TokenBreakdown";

// DeepSeek-style: raw input_tokens is non-cached-only; cached_input aliases cache_read.
const tk: TokenBreakdownDto = {
  prompt_input_total_tokens: 1100,
  prompt_input_non_cached_tokens: 1000,
  cache_read_tokens: 100,
  cache_write_tokens: 0,
  cache_hit_rate: 0.0909,
  input_tokens: 1000,
  output_tokens: 50,
  cached_input_tokens: 100,
  reasoning_tokens: null,
  total_tokens: 1150,
};

describe("TokenBreakdown", () => {
  afterEach(() => cleanup());

  it("renders the unified product metrics", () => {
    render(<TokenBreakdown tk={tk} />);
    expect(screen.getByText("Token Breakdown")).toBeDefined();
    expect(screen.getByText("Prompt Input (Total)")).toBeDefined();
    expect(screen.getByText("Cache Hit Rate")).toBeDefined();
    // A unified value renders (1,100 = prompt_input_total; distinct from the
    // raw fields which are 1,000 / 100).
    expect(screen.getByText("1,100")).toBeDefined();
  });

  it("surfaces the raw audit fields (provider literal values)", () => {
    render(<TokenBreakdown tk={tk} />);
    expect(screen.getByText("Raw Audit")).toBeDefined();
    expect(screen.getByText("Input Tokens (raw)")).toBeDefined();
    expect(screen.getByText("Cached Input Tokens (raw)")).toBeDefined();
  });
});
