import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { LedgerTable } from "./LedgerTable";
import type { ColumnDef } from "@tanstack/react-table";

interface TestRow {
  id: string;
  name: string;
}

const columns: Array<ColumnDef<TestRow>> = [
  { accessorKey: "name", header: "Name" },
];

const rows: TestRow[] = [
  { id: "1", name: "Alpha" },
  { id: "2", name: "Beta" },
  { id: "3", name: "Gamma" },
];

describe("LedgerTable", () => {
  afterEach(cleanup);

  it("renders rows", () => {
    render(<LedgerTable ariaLabel="Test table" columns={columns} rows={rows} />);
    expect(screen.getByText("Alpha")).toBeDefined();
    expect(screen.getByText("Beta")).toBeDefined();
    expect(screen.getByText("Gamma")).toBeDefined();
  });

  it("calls onSelect when a row is clicked", async () => {
    const onSelect = vi.fn();
    render(<LedgerTable ariaLabel="Test table" columns={columns} rows={rows} onSelect={onSelect} />);
    await userEvent.click(screen.getByText("Alpha"));
    expect(onSelect).toHaveBeenCalledWith(rows[0]);
  });

  it("applies is-selected class to selected row", () => {
    render(<LedgerTable ariaLabel="Test table" columns={columns} rows={rows} selectedId="2" />);
    const row = screen.getByText("Beta").closest("tr");
    expect(row?.className).toContain("is-selected");
  });

  it("selects row on Enter key", async () => {
    const onSelect = vi.fn();
    render(<LedgerTable ariaLabel="Test table" columns={columns} rows={rows} onSelect={onSelect} />);
    const cell = screen.getByText("Alpha");
    await userEvent.type(cell, "{enter}");
    expect(onSelect).toHaveBeenCalledWith(rows[0]);
  });

  it("selects row on Space key", async () => {
    const onSelect = vi.fn();
    render(<LedgerTable ariaLabel="Test table" columns={columns} rows={rows} onSelect={onSelect} />);
    const cell = screen.getByText("Alpha");
    await userEvent.type(cell, " ");
    expect(onSelect).toHaveBeenCalledWith(rows[0]);
  });

  it("does not call onSelect for disabled rows", async () => {
    const onSelect = vi.fn();
    const isDisabled = (row: TestRow) => row.id === "1";
    render(
      <LedgerTable
        ariaLabel="Test table"
        columns={columns}
        rows={rows}
        onSelect={onSelect}
        disabledPredicate={isDisabled}
      />,
    );
    await userEvent.click(screen.getByText("Alpha"));
    expect(onSelect).not.toHaveBeenCalled();
  });

  it("does not call onSelect for disabled rows on keyboard Enter", async () => {
    const onSelect = vi.fn();
    const isDisabled = (row: TestRow) => row.id === "1";
    render(
      <LedgerTable
        ariaLabel="Test table"
        columns={columns}
        rows={rows}
        onSelect={onSelect}
        disabledPredicate={isDisabled}
      />,
    );
    const cell = screen.getByText("Alpha");
    await userEvent.type(cell, "{enter}");
    expect(onSelect).not.toHaveBeenCalled();
  });

  it("renders empty slot when no rows", () => {
    render(
      <LedgerTable
        ariaLabel="Test table"
        columns={columns}
        rows={[]}
        emptySlot={<div>Nothing here</div>}
      />,
    );
    expect(screen.getByText("Nothing here")).toBeDefined();
  });

  it("renders loading slot when loading", () => {
    render(
      <LedgerTable
        ariaLabel="Test table"
        columns={columns}
        rows={[]}
        isLoading
        loadingSlot={<div>Loading...</div>}
      />,
    );
    expect(screen.getByText("Loading...")).toBeDefined();
  });

  it("applies is-readonly class when readOnly is true", () => {
    render(<LedgerTable ariaLabel="Test table" columns={columns} rows={rows} readOnly />);
    const table = document.querySelector(".ledger-table");
    expect(table?.className).toContain("is-readonly");
  });

  it("does not call onSelect when readOnly", async () => {
    const onSelect = vi.fn();
    render(
      <LedgerTable ariaLabel="Test table" columns={columns} rows={rows} readOnly onSelect={onSelect} />,
    );
    await userEvent.click(screen.getByText("Alpha"));
    expect(onSelect).not.toHaveBeenCalled();
  });

  it("does not call onSelect on keyboard Enter when readOnly", async () => {
    const onSelect = vi.fn();
    render(
      <LedgerTable ariaLabel="Test table" columns={columns} rows={rows} readOnly onSelect={onSelect} />,
    );
    await userEvent.type(screen.getByText("Alpha"), "{enter}");
    expect(onSelect).not.toHaveBeenCalled();
  });

  it("sets tabIndex -1 on rows when readOnly", () => {
    render(<LedgerTable ariaLabel="Test table" columns={columns} rows={rows} readOnly />);
    const row = screen.getByText("Alpha").closest("tr");
    expect(row?.getAttribute("tabindex")).toBe("-1");
  });
});
