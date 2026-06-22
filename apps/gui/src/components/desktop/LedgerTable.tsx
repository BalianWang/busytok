import { flexRender, getCoreRowModel, useReactTable, type ColumnDef } from "@tanstack/react-table";
import type { ReactNode } from "react";

interface LedgerTableProps<T extends { id: string }> {
  ariaLabel: string;
  columns: Array<ColumnDef<T>>;
  rows: T[];
  selectedId?: string | null;
  onSelect?: (row: T) => void;
  disabledPredicate?: (row: T) => boolean;
  readOnly?: boolean;
  emptySlot?: ReactNode;
  loadingSlot?: ReactNode;
  isLoading?: boolean;
}

export function LedgerTable<T extends { id: string }>({
  ariaLabel,
  columns,
  rows,
  selectedId,
  onSelect,
  disabledPredicate,
  readOnly,
  emptySlot,
  loadingSlot,
  isLoading,
}: LedgerTableProps<T>) {
  const table = useReactTable({ data: rows, columns, getCoreRowModel: getCoreRowModel() });

  return (
    <div className="ledger-table-wrapper">
      <table className={`ledger-table${readOnly ? " is-readonly" : ""}`} aria-label={ariaLabel}>
        <thead>
          {table.getHeaderGroups().map((headerGroup) => (
            <tr key={headerGroup.id}>
              {headerGroup.headers.map((header) => (
                <th key={header.id}>
                  {header.isPlaceholder
                    ? null
                    : flexRender(header.column.columnDef.header, header.getContext())}
                </th>
              ))}
            </tr>
          ))}
        </thead>
        <tbody>
          {table.getRowModel().rows.map((row) => {
            const isDisabled = disabledPredicate?.(row.original) ?? false;
            return (
              <tr
                key={row.id}
                className={
                  [
                    row.original.id === selectedId ? "is-selected" : null,
                    isDisabled ? "is-disabled" : null,
                  ]
                    .filter(Boolean)
                    .join(" ") || undefined
                }
                aria-selected={row.original.id === selectedId ? true : undefined}
                onClick={() => {
                  if (!readOnly && !isDisabled) onSelect?.(row.original);
                }}
                onKeyDown={(event) => {
                  if ((event.key === "Enter" || event.key === " ") && !readOnly && !isDisabled) {
                    event.preventDefault();
                    onSelect?.(row.original);
                  }
                }}
                tabIndex={readOnly || isDisabled ? -1 : 0}
              >
                {row.getVisibleCells().map((cell) => (
                  <td key={cell.id}>
                    {flexRender(cell.column.columnDef.cell, cell.getContext())}
                  </td>
                ))}
              </tr>
            );
          })}
        </tbody>
      </table>

      {isLoading && (loadingSlot ?? <div className="ledger-table__slot">Loading...</div>)}
      {!isLoading && rows.length === 0 && (emptySlot ?? <div className="ledger-table__slot">No data.</div>)}
    </div>
  );
}
