import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { ReceiptPaper } from "./ReceiptPaper";
import { toReceiptViewModel } from "./viewModel";
import {
  MANY_MODELS,
  LONG_NAMES,
  NO_DATA,
  NORMAL_DAY,
  OTHERS_ALL_UNAVAILABLE,
  OTHERS_MIXED_COST,
  PARTIAL_COST,
  ZERO_COST,
} from "./fixtures";

afterEach(() => cleanup());

function renderVm(dto = NORMAL_DAY) {
  return render(<ReceiptPaper vm={toReceiptViewModel(dto)} />);
}

describe("ReceiptPaper", () => {
  it("renders brand, summary, ITEMS, and a TOTAL block", () => {
    renderVm();
    expect(screen.getByText("BUSYTOK")).toBeDefined();
    expect(screen.getByText("ITEM")).toBeDefined();
    expect(screen.getByText("TOTAL")).toBeDefined();
    // The hero block was removed; summary line carries the split.
    expect(screen.getByText(/cache hit/)).toBeDefined();
    // Serial follows the #MMDD-XXXX receipt convention.
    expect(screen.getByText(/RECEIPT #0626-[0-9A-F]{4}/)).toBeDefined();
  });

  it("does NOT render the old oversized TOTAL TOKENS hero block", () => {
    const { container } = renderVm();
    expect(screen.queryByText("TOTAL TOKENS")).toBeNull();
    // The hero block's CSS hook must also be gone from the DOM.
    expect(container.querySelector(".receipt__hero")).toBeNull();
  });

  it("renders an OTHERS row when more than 5 models", () => {
    renderVm(MANY_MODELS);
    expect(screen.getByText(/OTHERS \(3\)/)).toBeDefined();
    // All 3 overflow models are exact → OTHERS cost is a plain $X.XX (no ≈).
    // Symmetric with OTHERS_MIXED_COST which renders ≈$X.XX.
    expect(screen.getByText("$1.60")).toBeDefined();
  });

  it("renders OTHERS as — (not ≈$0.00) when all overflow models are unavailable", () => {
    renderVm(OTHERS_ALL_UNAVAILABLE); // top 5 exact; overflow (2) all unavailable
    expect(screen.getAllByText("—").length).toBe(1); // the OTHERS row cost only
  });

  it("renders OTHERS as ≈$X.XX when overflow mixes exact + unavailable", () => {
    renderVm(OTHERS_MIXED_COST); // overflow: 1 exact ($1.50) + 1 unavailable → partial
    // $1.50 (the paid-overflow cost; free-overflow contributes 0)
    expect(screen.getByText("≈$1.50")).toBeDefined(); // OTHERS row cost
  });

  it("marks partial aggregate cost with ≈ and keeps exact item cost plain", () => {
    renderVm(PARTIAL_COST); // aggregate cost_status partial (47.21); one exact item ($24.10)
    expect(screen.getByText("≈$47.21")).toBeDefined(); // TOTAL block
    expect(screen.getByText("$24.10")).toBeDefined(); // exact-status item row
    expect(screen.getAllByText("—").length).toBeGreaterThan(0); // unavailable item
  });

  it("shows the empty state when there are no models and no tokens", () => {
    const { container } = renderVm(NO_DATA);
    expect(container.querySelector(".receipt-paper__empty")).not.toBeNull();
    // The meta row still renders above the empty block; peak_hour null → "PEAK —".
    expect(screen.getByText("PEAK —")).toBeDefined();
  });

  it("renders 'cache hit --' when cache_hit_rate is null but tokens exist", () => {
    // ZERO_COST: total_tokens > 0 (inherited), cache_hit_rate null, cost unavailable.
    // Non-empty path → summary block renders with the null-rate fallback "--".
    renderVm(ZERO_COST);
    expect(screen.getByText(/cache hit --/)).toBeDefined();
    // TOTAL cost is unavailable → "—".
    expect(screen.getByText("TOTAL")).toBeDefined();
  });

  it("truncates long model names", () => {
    renderVm(LONG_NAMES);
    const name = screen.getByText(/claude-sonnet-4-5-thinking-very-long/);
    expect(name).toBeDefined();
  });

  it("aligns ITEMS and TOTAL on the same three-column structure", () => {
    // jsdom does not resolve grid-template-columns from external CSS, so we
    // assert the DOM contract instead: every item row and the TOTAL row must
    // carry exactly three children in the (name, tokens, cost) order, so the
    // CSS grid (declared in receipt.css) lines them up column-for-column.
    const { container } = renderVm();
    const itemRows = container.querySelectorAll(".receipt__item");
    const totalRow = container.querySelector(".receipt__total");
    expect(itemRows.length).toBeGreaterThan(0);
    expect(totalRow).not.toBeNull();
    const itemChildren = Array.from(itemRows[0].children);
    const totalChildren = Array.from(totalRow!.children);
    expect(itemChildren.length).toBe(3);
    expect(totalChildren.length).toBe(3);
    // Class-name order encodes the column role so CSS can target each track.
    const itemRoles = itemChildren.map((c) => c.className);
    const totalRoles = totalChildren.map((c) => c.className);
    expect(itemRoles[0]).toBe("receipt__item-name");
    expect(itemRoles[1]).toBe("receipt__item-tokens");
    expect(itemRoles[2]).toBe("receipt__item-cost");
    expect(totalRoles[0]).toBe("receipt__total-label");
    expect(totalRoles[1]).toBe("receipt__total-tokens");
    expect(totalRoles[2]).toBe("receipt__total-cost");
  });
});
