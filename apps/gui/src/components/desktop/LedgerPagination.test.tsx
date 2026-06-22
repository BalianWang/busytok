import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { LedgerPagination } from "./LedgerPagination";

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

const baseProps = {
  pageSize: 100,
  hasPrev: false,
  hasNext: true,
  totalCount: null,
  visibleStart: 1,
  visibleEnd: 25,
  pageIndex: 0,
  loading: false,
  onPrev: vi.fn(),
  onNext: vi.fn(),
  onPageSizeChange: vi.fn(),
};

describe("LedgerPagination", () => {
  it("renders rows-per-page selector with active state", () => {
    render(<LedgerPagination {...baseProps} />);
    expect(screen.getByText("Rows")).toBeDefined();
    expect(screen.getByText("25")).toBeDefined();
    expect(screen.getByText("50")).toBeDefined();
    expect(screen.getByText("100").className).toContain("is-active");
  });

  it("renders Prev and Next buttons", () => {
    render(<LedgerPagination {...baseProps} />);
    expect(screen.getByText("Prev")).toBeDefined();
    expect(screen.getByText("Next")).toBeDefined();
  });

  it("disables Prev when hasPrev is false", () => {
    render(<LedgerPagination {...baseProps} hasPrev={false} />);
    expect(screen.getByText("Prev").closest("button")).toHaveProperty("disabled", true);
  });

  it("disables Next when hasNext is false", () => {
    render(<LedgerPagination {...baseProps} hasNext={false} />);
    expect(screen.getByText("Next").closest("button")).toHaveProperty("disabled", true);
  });

  it("disables both buttons when loading", () => {
    render(<LedgerPagination {...baseProps} loading />);
    const prev = screen.getByText("Prev").closest("button")!;
    const next = screen.getByText("Next").closest("button")!;
    expect(prev.disabled).toBe(true);
    expect(next.disabled).toBe(true);
  });

  it("shows range with totalCount when available", () => {
    render(
      <LedgerPagination
        {...baseProps}
        totalCount={243}
        visibleStart={1}
        visibleEnd={25}
      />,
    );
    expect(screen.getByText("Showing 1–25 of 243")).toBeDefined();
  });

  it("shows range without total when totalCount is null", () => {
    render(
      <LedgerPagination
        {...baseProps}
        totalCount={null}
        visibleStart={1}
        visibleEnd={25}
      />,
    );
    expect(screen.getByText("Showing 1–25")).toBeDefined();
  });

  it("shows degraded summary when visibleStart is null", () => {
    render(
      <LedgerPagination
        {...baseProps}
        totalCount={null}
        visibleStart={null}
        visibleEnd={25}
      />,
    );
    expect(screen.getByText("Showing 25 items")).toBeDefined();
  });

  it("shows page number from pageIndex without totalCount", () => {
    render(
      <LedgerPagination
        {...baseProps}
        totalCount={null}
        pageIndex={1}
        visibleStart={101}
        visibleEnd={125}
      />,
    );
    expect(screen.getByText("Page 2")).toBeDefined();
  });

  it("shows page/total when totalCount is available", () => {
    render(
      <LedgerPagination
        {...baseProps}
        totalCount={243}
        pageIndex={1}
        visibleStart={101}
        visibleEnd={125}
      />,
    );
    expect(screen.getByText("2 / 3")).toBeDefined();
  });

  it("calls onPageSizeChange when a different size is clicked", async () => {
    const onPageSizeChange = vi.fn();
    const user = userEvent.setup();
    render(
      <LedgerPagination
        {...baseProps}
        onPageSizeChange={onPageSizeChange}
      />,
    );
    await user.click(screen.getByText("25"));
    expect(onPageSizeChange).toHaveBeenCalledWith(25);
  });

  it("calls onPrev when Prev is clicked", async () => {
    const onPrev = vi.fn();
    const user = userEvent.setup();
    render(<LedgerPagination {...baseProps} hasPrev onPrev={onPrev} />);
    await user.click(screen.getByText("Prev"));
    expect(onPrev).toHaveBeenCalled();
  });

  it("calls onNext when Next is clicked", async () => {
    const onNext = vi.fn();
    const user = userEvent.setup();
    render(<LedgerPagination {...baseProps} onNext={onNext} />);
    await user.click(screen.getByText("Next"));
    expect(onNext).toHaveBeenCalled();
  });
});
