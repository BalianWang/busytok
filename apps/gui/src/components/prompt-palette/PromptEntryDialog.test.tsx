import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { useState } from "react";
import type { PromptEntryDto } from "@busytok/protocol-types";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { PromptEntryDialog } from "./PromptEntryDialog";

function makeEntry(overrides: Partial<PromptEntryDto> = {}): PromptEntryDto {
  return {
    id: "prompt-1",
    content: "  Keep surrounding whitespace.\n\n",
    alias: null,
    tags: [],
    is_pinned: false,
    usage_count: 0,
    last_used_at_ms: null,
    created_at_ms: 1715900000000,
    updated_at_ms: 1716000000000,
    ...overrides,
  };
}

function DialogHarness({ onSubmit = vi.fn() }: { onSubmit?: ReturnType<typeof vi.fn> }) {
  const [open, setOpen] = useState(false);

  return (
    <>
      <button type="button" onClick={() => setOpen(true)}>
        Open dialog
      </button>
      <PromptEntryDialog
        open={open}
        onClose={() => setOpen(false)}
        onSubmit={onSubmit}
      />
    </>
  );
}

describe("PromptEntryDialog", () => {
  afterEach(() => {
    cleanup();
  });

  it("requires content before submit", async () => {
    const user = userEvent.setup();
    const onSubmit = vi.fn();

    render(
      <PromptEntryDialog
        open={true}
        onClose={() => {}}
        onSubmit={onSubmit}
      />,
    );

    await user.click(screen.getByRole("button", { name: "Save" }));

    expect(onSubmit).not.toHaveBeenCalled();
    expect(screen.getByText("Content is required.")).toBeDefined();
  });

  it("disables native text assistance on the tags input", () => {
    render(
      <PromptEntryDialog
        open={true}
        onClose={() => {}}
        onSubmit={vi.fn()}
      />,
    );

    const input = screen.getByLabelText("Tags") as HTMLInputElement;
    expect(input.getAttribute("autocomplete")).toBe("off");
    expect(input.getAttribute("autocorrect")).toBe("off");
    expect(input.getAttribute("autocapitalize")).toBe("off");
    expect(input.getAttribute("data-form-type")).toBe("other");
    expect(input.getAttribute("spellcheck")).toBe("false");
  });

  it("submits content with alias and parsed tags", async () => {
    const user = userEvent.setup();
    const onSubmit = vi.fn();
    const content = "  Turn bullets into notes.\n\nKeep closing space.  ";

    render(
      <PromptEntryDialog
        open={true}
        onClose={() => {}}
        onSubmit={onSubmit}
      />,
    );

    await user.type(screen.getByLabelText("Content"), content);
    await user.type(screen.getByLabelText("Alias"), "  release  ");
    await user.type(screen.getByLabelText("Tags"), "release, writing, release");
    await user.click(screen.getByRole("button", { name: "Save" }));

    expect(onSubmit).toHaveBeenCalledWith({
      content,
      alias: "release",
      tags: ["release", "writing"],
    });
  });

  it("validates alias max length and forbidden characters", async () => {
    const user = userEvent.setup();
    const onSubmit = vi.fn();

    render(
      <PromptEntryDialog
        open={true}
        onClose={() => {}}
        onSubmit={onSubmit}
      />,
    );

    // Fill content so the only blocker is alias validation
    await user.type(screen.getByLabelText("Content"), "Valid content");

    // Save alias input ref before validation re-render
    const aliasInput = screen.getByLabelText("Alias") as HTMLInputElement;

    // Alias too long
    await user.type(aliasInput, "a".repeat(81));
    await user.click(screen.getByRole("button", { name: "Save" }));
    expect(onSubmit).not.toHaveBeenCalled();
    expect(screen.getByText("Alias must be at most 80 characters.")).toBeDefined();

    // Replace with alias containing forbidden chars
    await user.clear(aliasInput);
    await user.type(aliasInput, "my alias");
    await user.click(screen.getByRole("button", { name: "Save" }));
    expect(onSubmit).not.toHaveBeenCalled();
    expect(screen.getByText("Alias must not contain whitespace, quotes, or backticks.")).toBeDefined();

    // Valid alias allows submit
    await user.clear(aliasInput);
    await user.type(aliasInput, "my-alias");
    await user.click(screen.getByRole("button", { name: "Save" }));
    expect(onSubmit).toHaveBeenCalled();
  });

  it("validates content max length", async () => {
    const user = userEvent.setup();
    const onSubmit = vi.fn();
    const longContent = "x".repeat(65537);

    render(
      <PromptEntryDialog
        open={true}
        onClose={() => {}}
        onSubmit={onSubmit}
      />,
    );

    const contentTextarea = screen.getByLabelText("Content");
    fireEvent.change(contentTextarea, { target: { value: longContent } });
    await user.click(screen.getByRole("button", { name: "Save" }));

    expect(onSubmit).not.toHaveBeenCalled();
    expect(screen.getByText("Content must be at most 65536 characters.")).toBeDefined();
  });

  it("displays server error when onSubmit throws", async () => {
    const user = userEvent.setup();
    const onSubmit = vi.fn().mockRejectedValue(new Error("Alias already exists"));

    render(
      <PromptEntryDialog
        open={true}
        onClose={() => {}}
        onSubmit={onSubmit}
      />,
    );

    await user.type(screen.getByLabelText("Content"), "Valid content");
    await user.type(screen.getByLabelText("Alias"), "duplicate");
    await user.click(screen.getByRole("button", { name: "Save" }));

    expect(screen.getByText("Alias already exists")).toBeDefined();
  });

  it("preserves existing prompt content when saved without edits", async () => {
    const user = userEvent.setup();
    const onSubmit = vi.fn();
    const entry = makeEntry();

    render(
      <PromptEntryDialog
        open={true}
        entry={entry}
        onClose={() => {}}
        onSubmit={onSubmit}
      />,
    );

    await user.click(screen.getByRole("button", { name: "Save" }));

    expect(onSubmit).toHaveBeenCalledWith(
      expect.objectContaining({
        content: entry.content,
      }),
    );
  });

  it("focuses the content field when opened", async () => {
    const user = userEvent.setup();
    render(<DialogHarness />);

    await user.click(screen.getByRole("button", { name: "Open dialog" }));

    expect(document.activeElement).toBe(screen.getByLabelText("Content"));
  });

  it("keeps keyboard focus inside the dialog with Tab and Shift+Tab", async () => {
    const user = userEvent.setup();
    render(
      <PromptEntryDialog
        open={true}
        onClose={() => {}}
        onSubmit={vi.fn()}
      />,
    );

    const closeButton = screen.getByRole("button", { name: "Close" });
    const saveButton = screen.getByRole("button", { name: "Save" });

    saveButton.focus();
    await user.tab();
    expect(document.activeElement).toBe(closeButton);

    closeButton.focus();
    await user.tab({ shift: true });
    expect(document.activeElement).toBe(saveButton);
  });

  it("closes on Escape and restores focus to the opener", async () => {
    const user = userEvent.setup();
    render(<DialogHarness />);

    const opener = screen.getByRole("button", { name: "Open dialog" });
    await user.click(opener);

    await user.keyboard("{Escape}");

    expect(screen.queryByRole("dialog")).toBeNull();
    expect(document.activeElement).toBe(opener);
  });
});
