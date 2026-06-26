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
} from "./fixtures";

afterEach(() => cleanup());

function renderVm(dto = NORMAL_DAY) {
  return render(<ReceiptPaper vm={toReceiptViewModel(dto)} />);
}

describe("ReceiptPaper", () => {
  it("renders brand, hero, and a TOTAL block", () => {
    renderVm();
    expect(screen.getByText("BUSYTOK")).toBeDefined();
    expect(screen.getByText("TOTAL TOKENS")).toBeDefined();
    expect(screen.getByText("ITEMS")).toBeDefined();
    expect(screen.getByText("TOTAL")).toBeDefined();
  });

  it("renders an OTHERS row when more than 5 models", () => {
    renderVm(MANY_MODELS);
    expect(screen.getByText(/OTHERS \(3\)/)).toBeDefined();
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
  });

  it("truncates long model names", () => {
    renderVm(LONG_NAMES);
    const name = screen.getByText(/claude-sonnet-4-5-thinking-very-long/);
    expect(name).toBeDefined();
  });
});
