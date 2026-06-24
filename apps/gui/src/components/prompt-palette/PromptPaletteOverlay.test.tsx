import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { readFileSync } from "node:fs";
import { pathToFileURL } from "node:url";
import type { PromptEntryDto } from "@busytok/protocol-types";
import { PromptPaletteOverlay } from "./PromptPaletteOverlay";

function makePrompt(overrides: Partial<PromptEntryDto> = {}): PromptEntryDto {
  return {
    id: "prompt-1",
    content: "Summarize this thread clearly.",
    alias: null,
    tags: ["writing"],
    is_pinned: true,
    usage_count: 3,
    last_used_at_ms: null,
    created_at_ms: 1715900000000,
    updated_at_ms: 1716000000000,
    ...overrides,
  };
}

function renderOverlay(props: Partial<React.ComponentProps<typeof PromptPaletteOverlay>> = {}) {
  const baseProps: React.ComponentProps<typeof PromptPaletteOverlay> = {
    open: true,
    entries: [
      makePrompt(),
      makePrompt({
        id: "prompt-2",
        alias: "changelog",
        content: "Draft a concise changelog.",
        is_pinned: false,
      }),
    ],
    query: "",
    onQueryChange: vi.fn(),
    onClose: vi.fn(),
    onExecute: vi.fn(),
    onOpenPage: vi.fn(),
    onCreateNew: vi.fn(),
    defaultAction: "CopyAndPaste",
    onEdit: vi.fn(),
    onTogglePin: vi.fn(),
    onDelete: vi.fn(),
    ...props,
  };

  return {
    user: userEvent.setup(),
    props: baseProps,
    ...render(<PromptPaletteOverlay {...baseProps} />),
  };
}

describe("PromptPaletteOverlay", () => {
  afterEach(() => {
    cleanup();
  });

  it("focuses search and Enter executes the selected prompt with its default action", async () => {
    const { user, props } = renderOverlay();

    const search = await screen.findByRole("searchbox", { name: /prompt palette search/i });
    expect(document.activeElement).toBe(search);

    await user.keyboard("{Enter}");

    expect(props.onExecute).toHaveBeenCalledWith(props.entries[0], "CopyAndPaste");
  });

  it("keeps the search field as the initial focus target", async () => {
    renderOverlay({ open: true });
    const search = screen.getByRole("searchbox", { name: /prompt palette search/i });
    // Autofocus runs on a microtask timer; wait for it to land.
    await waitFor(() => {
      expect(document.activeElement).toBe(search);
    });
  });

  it("exposes a consistent keycap vocabulary rather than ad-hoc pill text", () => {
    renderOverlay();

    // The close affordance must render as a semantic keycap element so the
    // overlay's keyboard hints share one vocabulary rather than plain text
    // or ad-hoc pill markup.
    const closeKeycap = document.querySelector(
      ".prompt-overlay__close.prompt-overlay__keycap",
    );
    expect(closeKeycap).not.toBeNull();
    expect(closeKeycap?.textContent).toBe("Esc");
  });

  it("does not render a keyboard-hints footer", () => {
    renderOverlay();

    expect(document.querySelector(".prompt-overlay__hints")).toBeNull();
  });

  it("marks the selected row with surface lift and a focus-edge accent cue", () => {
    renderOverlay();

    const rows = document.querySelectorAll(".prompt-overlay__row");
    const selected = rows[0];
    expect(selected.classList.contains("is-selected")).toBe(true);

    // Neutral-lift-first / accent-rail-second is a CSS contract pinned in
    // components.css. Assert the surface lift uses the neutral hover-strong
    // token (not an accent tint) and the ::before rail carries the accent,
    // so a refactor can't silently drop either layer of the selection cue.
    const css = readFileSync(
      pathToFileURL("./src/styles/components.css"),
      "utf8",
    );
    expect(css).toMatch(/\.prompt-overlay__row\.is-selected\s*{[\s\S]*?--color-hover-strong/);
    expect(css).toMatch(/\.prompt-overlay__row\.is-selected::before\s*{[\s\S]*?--color-accent-500/);
  });

  it("disables native text assistance on the palette searchbox", async () => {
    renderOverlay();

    const input = await screen.findByRole("searchbox", { name: /prompt palette search/i }) as HTMLInputElement;
    expect(input.getAttribute("autocomplete")).toBe("off");
    expect(input.getAttribute("autocorrect")).toBe("off");
    expect(input.getAttribute("autocapitalize")).toBe("off");
    expect(input.getAttribute("data-form-type")).toBe("other");
    expect(input.getAttribute("spellcheck")).toBe("false");
  });

  it("supports arrow navigation, Cmd/Ctrl+C copy, and Escape close", async () => {
    const { user, props } = renderOverlay();

    await user.keyboard("{ArrowDown}");
    await user.keyboard("{Meta>}c{/Meta}");
    await user.keyboard("{Escape}");

    expect(props.onExecute).toHaveBeenCalledWith(props.entries[1], "OnlyCopy");
    expect(props.onClose).toHaveBeenCalled();
  });

  it("opens actions with Cmd/Ctrl+K and exposes Edit", async () => {
    const { user, props } = renderOverlay();

    await user.keyboard("{Meta>}k{/Meta}");

    const menu = await screen.findByRole("menu", { name: /actions for/i });
    expect(menu).toBeDefined();
    await user.click(screen.getByRole("menuitem", { name: "Edit" }));

    expect(props.onEdit).toHaveBeenCalledWith(props.entries[0]);
  });

  it("can render as a standalone window without a backdrop", () => {
    renderOverlay({ presentation: "window" } as Partial<React.ComponentProps<typeof PromptPaletteOverlay>>);

    const shell = document.querySelector(".prompt-overlay__window-shell");
    const surface = document.querySelector(".prompt-overlay__surface--window");

    expect(document.querySelector(".prompt-overlay--window")).toBeNull();
    expect(shell).not.toBeNull();
    expect(surface).not.toBeNull();
    expect(shell?.children).toHaveLength(1);
    expect(shell?.firstElementChild).toBe(surface);
    expect(document.querySelector(".prompt-overlay__backdrop")).toBeNull();
  });

  it("keeps overlay mode rendered through a portal with a backdrop", () => {
    const host = document.createElement("div");
    document.body.appendChild(host);

    try {
      render(
        <PromptPaletteOverlay
          open
          entries={[makePrompt()]}
          query=""
          defaultAction="CopyAndPaste"
          onQueryChange={vi.fn()}
          onClose={vi.fn()}
          onExecute={vi.fn()}
          onOpenPage={vi.fn()}
          onCreateNew={vi.fn()}
        />,
        { container: host },
      );

      expect(host.querySelector(".prompt-overlay")).toBeNull();
      expect(document.body.querySelector(".prompt-overlay__backdrop")).not.toBeNull();
    } finally {
      host.remove();
    }
  });

  it("executes all available action menu commands for the selected prompt", async () => {
    const { user, props } = renderOverlay();
    const openActions = () => {
      fireEvent.keyDown(screen.getByRole("dialog", { name: "Prompt Palette" }), {
        key: "k",
        metaKey: true,
      });
    };

    openActions();
    await user.click(await screen.findByRole("menuitem", { name: "Copy" }));
    await waitFor(() => {
      expect(props.onExecute).toHaveBeenCalledWith(props.entries[0], "OnlyCopy");
    });

    openActions();
    await user.click(await screen.findByRole("menuitem", { name: "Paste" }));
    expect(props.onExecute).toHaveBeenCalledWith(props.entries[0], "CopyAndPaste");

    openActions();
    await user.click(await screen.findByRole("menuitem", { name: "Edit" }));
    expect(props.onEdit).toHaveBeenCalledWith(props.entries[0]);

    expect(screen.queryByRole("menuitem", { name: "Duplicate" })).toBeNull();

    openActions();
    await user.click(await screen.findByRole("menuitem", { name: "Unpin" }));
    expect(props.onTogglePin).toHaveBeenCalledWith(props.entries[0]);

    openActions();
    await user.click(await screen.findByRole("menuitem", { name: "Delete" }));
    expect(props.onDelete).toHaveBeenCalledWith(props.entries[0]);

    openActions();
    await user.click(await screen.findByRole("menuitem", { name: "Open in Prompt Palette Page" }));
    expect(props.onOpenPage).toHaveBeenCalledOnce();
  });

  it("keeps disabled optional action menu commands inert", async () => {
    const { user, props } = renderOverlay({
      onDelete: undefined,
      onEdit: undefined,
      onTogglePin: undefined,
    });

    await user.keyboard("{Meta>}k{/Meta}");
    await user.click(await screen.findByRole("menuitem", { name: "Edit" }));
    await user.click(screen.getByRole("menuitem", { name: "Unpin" }));
    await user.click(screen.getByRole("menuitem", { name: "Delete" }));

    expect(props.onEdit).toBeUndefined();
    expect(props.onTogglePin).toBeUndefined();
    expect(props.onDelete).toBeUndefined();
  });

  it("calls create new with Cmd/Ctrl+N", async () => {
    const { user, props } = renderOverlay();

    await user.keyboard("{Meta>}n{/Meta}");

    expect(props.onCreateNew).toHaveBeenCalled();
  });

  it("shows an empty state without seeding prompts", () => {
    renderOverlay({ entries: [], query: "missing" });

    expect(screen.getByText("No matches")).toBeDefined();
    expect(screen.queryByText("Summarize a thread")).toBeNull();
  });

  it("shows initial, loading, error, and external status states", () => {
    const { rerender, props } = renderOverlay({
      entries: [],
      query: "",
      statusMessage: "Copied to clipboard.",
    });

    expect(screen.getByText("No saved prompts")).toBeDefined();
    expect(screen.getByText("Saved prompts will appear here.")).toBeDefined();
    expect(screen.getByRole("status").textContent).toBe("Copied to clipboard.");

    rerender(<PromptPaletteOverlay {...props} entries={[]} isLoading />);
    expect(screen.getByText("Loading prompts...")).toBeDefined();

    rerender(<PromptPaletteOverlay {...props} entries={[]} error="Search failed" />);
    expect(screen.getByText("Search failed")).toBeDefined();
  });

  it("renders the compact palette chrome with a single search row and lighter metadata", () => {
    renderOverlay({
      entries: [
        makePrompt({
          usage_count: 3,
          last_used_at_ms: Date.now() - 5 * 60_000,
        }),
      ],
    });

    expect(screen.queryByText("Prompt Palette")).toBeNull();
    expect(screen.queryByText("Launch a saved prompt")).toBeNull();
    expect(screen.queryByText("Search prompts", { selector: "span" })).toBeNull();
    expect(screen.getByPlaceholderText("Search prompts")).toBeDefined();
    expect(screen.getByRole("button", { name: "Close Prompt Palette" }).textContent).toBe("Esc");
    expect(screen.getByText(/Summarize this thread clearly\./i, { selector: ".prompt-overlay__title" })).toBeDefined();
    expect(screen.getByText(/writing/i)).toBeDefined();
    expect(screen.queryByText("3 uses")).toBeNull();
    expect(screen.queryByText(/5m ago/i)).toBeNull();
    expect(screen.queryByText("Enter Default action")).toBeNull();
  });

  it("renders title from promptDisplayTitle (alias only, no duplicated content)", () => {
    renderOverlay();

    // Prompt-2 has alias "changelog" and content "Draft a concise changelog."
    // Under the new layout the title shows just the alias; the body is not rendered in the row.
    const title = screen.getByText("changelog", { selector: ".prompt-overlay__title" });
    expect(title).toBeDefined();

    const row = title.closest("button");
    expect(row?.textContent).not.toContain("Draft a concise changelog.");
  });

  it("renders tags inside the accessory container, capped at 3 with +N overflow", () => {
    renderOverlay({
      entries: [
        makePrompt({
          id: "many-tags",
          alias: "multi",
          content: "body",
          is_pinned: false,
          tags: ["a", "b", "c", "d", "e"],
        }),
      ],
    });

    const accessory = document.querySelector(".prompt-overlay__accessory");
    expect(accessory).not.toBeNull();
    expect(accessory?.textContent).toContain("a • b • c");
    expect(accessory?.textContent).toContain("+2");
    expect(accessory?.textContent).not.toContain("d");
    expect(accessory?.textContent).not.toContain("e");
  });

  it("renders pinned chip inside the accessory container", () => {
    renderOverlay();

    // Prompt-1 (default) has is_pinned: true
    const accessory = document.querySelector(".prompt-overlay__accessory");
    expect(accessory).not.toBeNull();
    const pin = accessory!.querySelector(".prompt-overlay__pin");
    expect(pin).not.toBeNull();
  });

  it("does not render a preview line", () => {
    renderOverlay();

    expect(document.querySelector(".prompt-overlay__preview")).toBeNull();
    expect(document.querySelector(".prompt-overlay__titleline")).toBeNull();
  });

  it("applies is-selected to the row at selectedIndex", () => {
    renderOverlay();

    // selectedIndex initializes to 0 via useState, so the first row is selected at mount.
    const rows = document.querySelectorAll(".prompt-overlay__row");
    expect(rows[0].classList.contains("is-selected")).toBe(true);
    expect(rows[1].classList.contains("is-selected")).toBe(false);
  });

  it("moves is-selected to the next row on ArrowDown", () => {
    const { user } = renderOverlay();
    const searchInput = screen.getByRole("searchbox", { name: /prompt palette search/i });

    searchInput.focus();
    fireEvent.keyDown(searchInput, { key: "ArrowDown" });

    const rows = document.querySelectorAll(".prompt-overlay__row");
    expect(rows[0].classList.contains("is-selected")).toBe(false);
    expect(rows[1].classList.contains("is-selected")).toBe(true);
  });

  it("renders +1 overflow suffix at the 4-tag boundary", () => {
    renderOverlay({
      entries: [
        makePrompt({
          id: "four-tags",
          alias: "four",
          content: "body",
          is_pinned: false,
          tags: ["a", "b", "c", "d"],
        }),
      ],
    });

    const accessory = document.querySelector(".prompt-overlay__accessory");
    expect(accessory?.textContent).toContain("a • b • c");
    expect(accessory?.textContent).toContain("+1");
    expect(accessory?.textContent).not.toContain("d");
  });

  it("renders no tags span when tags array is empty", () => {
    renderOverlay({
      entries: [
        makePrompt({
          id: "no-tags",
          alias: "no-tags",
          content: "body",
          is_pinned: false,
          tags: [],
        }),
      ],
    });

    expect(document.querySelector(".prompt-overlay__tags")).toBeNull();
  });

  it("renders pinned chip and tags together with pinned first", () => {
    renderOverlay({
      entries: [
        makePrompt({
          id: "pinned-with-tags",
          alias: "combo",
          content: "body",
          is_pinned: true,
          tags: ["x", "y"],
        }),
      ],
    });

    const accessory = document.querySelector(".prompt-overlay__accessory");
    expect(accessory).not.toBeNull();
    const children = Array.from(accessory!.children);
    expect(children).toHaveLength(2);
    expect(children[0].classList.contains("prompt-overlay__pin")).toBe(true);
    expect(children[1].classList.contains("prompt-overlay__tags")).toBe(true);
  });

  it("truncates long no-alias content with ellipsis in the title", () => {
    const longBody = "a".repeat(120);
    renderOverlay({
      entries: [
        makePrompt({
          id: "long-body",
          alias: null,
          content: longBody,
        }),
      ],
    });

    const title = screen.getByText(/a+…$/, { selector: ".prompt-overlay__title" });
    expect(title).toBeDefined();
    // 80-char limit per promptDisplayTitle's truncation rule
    expect(title.textContent).toHaveLength(80);
    expect(title.textContent?.endsWith("…")).toBe(true);
  });

  it("clicking a row executes that row even before hover selection changes", async () => {
    const { props } = renderOverlay();

    const secondRow = screen.getByText("changelog", { selector: ".prompt-overlay__title" }).closest("button");
    expect(secondRow).not.toBeNull();
    fireEvent.click(secondRow!);

    expect(props.onExecute).toHaveBeenCalledWith(props.entries[1], "CopyAndPaste");
  });

  it("shows sanitized action feedback when execution rejects", async () => {
    const secret = "clipboard backend leaked private text";
    const { user } = renderOverlay({
      onExecute: vi.fn().mockRejectedValue(new Error(secret)),
    });

    await user.keyboard("{Meta>}c{/Meta}");

    expect(await screen.findByText("Could not run prompt action. Try again.")).toBeDefined();
    expect(screen.queryByText(secret)).toBeNull();
  });

  it("wraps Tab focus inside the overlay", async () => {
    renderOverlay();
    const searchInput = screen.getByRole("searchbox", { name: /prompt palette search/i });
    const secondRow = screen.getByText("changelog", { selector: ".prompt-overlay__title" }).closest("button")!;

    secondRow.focus();
    fireEvent.keyDown(secondRow, { key: "Tab" });

    expect(document.activeElement).toBe(searchInput);
  });

  it("wraps Shift+Tab from the first focusable control to the last", async () => {
    renderOverlay();
    const searchInput = screen.getByRole("searchbox", { name: /prompt palette search/i });
    const secondRow = screen.getByText("changelog", { selector: ".prompt-overlay__title" }).closest("button")!;

    searchInput.focus();
    fireEvent.keyDown(searchInput, { key: "Tab", shiftKey: true });

    expect(document.activeElement).toBe(secondRow);
  });

  it("moves focus back inside when Tab starts outside the overlay", async () => {
    renderOverlay();
    const outside = document.createElement("button");
    outside.textContent = "outside";
    document.body.appendChild(outside);
    outside.focus();

    fireEvent.keyDown(screen.getByRole("dialog", { name: "Prompt Palette" }), { key: "Tab" });

    expect(document.activeElement).toBe(screen.getByRole("searchbox", { name: /prompt palette search/i }));
    outside.remove();
  });

  it("returns no portal content while closed", () => {
    const { container } = renderOverlay({ open: false });

    expect(container.innerHTML).toBe("");
    expect(screen.queryByRole("dialog", { name: "Prompt Palette" })).toBeNull();
  });

  it("restores focus to the opener when closed", async () => {
    const opener = document.createElement("button");
    opener.textContent = "Open palette";
    document.body.appendChild(opener);
    opener.focus();

    const { unmount } = renderOverlay();
    await screen.findByRole("searchbox", { name: /prompt palette search/i });

    unmount();

    expect(document.activeElement).toBe(opener);
    opener.remove();
  });
});
