import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ConfirmDialog } from "./ConfirmDialog";

afterEach(() => cleanup());

describe("ConfirmDialog", () => {
  it("renders confirm and cancel actions", () => {
    render(
      <ConfirmDialog
        open
        title="Delete"
        body="Body"
        confirmLabel="Delete"
        onConfirm={vi.fn()}
        onCancel={vi.fn()}
      />,
    );

    expect(screen.getByRole("button", { name: "Cancel" })).toBeDefined();
    expect(screen.getByRole("button", { name: "Delete" })).toBeDefined();
  });

  it("renders the dialog title and body when open", () => {
    render(
      <ConfirmDialog
        open
        title="Delete prompt?"
        body="This cannot be undone."
        confirmLabel="Delete"
        onConfirm={vi.fn()}
        onCancel={vi.fn()}
      />,
    );

    expect(screen.getByText("Delete prompt?")).toBeDefined();
    expect(screen.getByText("This cannot be undone.")).toBeDefined();
  });

  it("renders nothing when closed", () => {
    render(
      <ConfirmDialog
        open={false}
        title="Delete"
        body="Body"
        confirmLabel="Delete"
        onConfirm={vi.fn()}
        onCancel={vi.fn()}
      />,
    );

    expect(screen.queryByRole("button", { name: "Delete" })).toBeNull();
  });

  it("marks the confirm action with the danger role", () => {
    render(
      <ConfirmDialog
        open
        title="Delete"
        body="Body"
        confirmLabel="Delete"
        onConfirm={vi.fn()}
        onCancel={vi.fn()}
      />,
    );

    // Semantic role lives on the className, not on aria; tests should
    // preserve this contract through token migration.
    expect(screen.getByRole("button", { name: "Delete" }).className).toContain("btn--danger");
  });
});
