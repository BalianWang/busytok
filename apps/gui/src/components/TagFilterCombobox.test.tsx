import { cleanup, render, screen, waitFor, act } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { busytokClient } from "../api/busytokClient";
import { TagFilterCombobox } from "./TagFilterCombobox";

function renderCombobox(props: {
  appliedTag?: string;
  onApplyTag?: (tag: string) => void;
  onClear?: () => void;
}) {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return render(
    <QueryClientProvider client={queryClient}>
      <TagFilterCombobox
        appliedTag={props.appliedTag ?? ""}
        onApplyTag={props.onApplyTag ?? (() => {})}
        onClear={props.onClear ?? (() => {})}
        placeholder="Filter by tag"
      />
    </QueryClientProvider>,
  );
}

describe("TagFilterCombobox", () => {
  beforeEach(() => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    vi.spyOn(busytokClient, "promptsSuggestTags").mockResolvedValue({
      tags: ["refactor", "release", "review"],
    });
  });

  afterEach(() => {
    cleanup();
    vi.useRealTimers();
    vi.restoreAllMocks();
  });

  it("renders an input with the applied tag value", () => {
    renderCombobox({ appliedTag: "review" });
    const input = screen.getByRole("combobox", { name: "Filter by tag" });
    expect(input).toBeDefined();
    expect((input as HTMLInputElement).value).toBe("review");
  });

  it("disables native text assistance while keeping the custom combobox UX", () => {
    renderCombobox({ appliedTag: "" });
    const input = screen.getByRole("combobox", { name: "Filter by tag" }) as HTMLInputElement;

    expect(input.getAttribute("autocomplete")).toBe("off");
    expect(input.getAttribute("autocorrect")).toBe("off");
    expect(input.getAttribute("autocapitalize")).toBe("off");
    expect(input.getAttribute("data-form-type")).toBe("other");
    expect(input.getAttribute("spellcheck")).toBe("false");
  });

  it("shows clear button when a tag is applied", () => {
    renderCombobox({ appliedTag: "review" });
    expect(screen.getByRole("button", { name: "Clear tag filter" })).toBeDefined();
  });

  it("does not show clear button when no tag is applied", () => {
    renderCombobox({ appliedTag: "" });
    expect(screen.queryByRole("button", { name: "Clear tag filter" })).toBeNull();
  });

  it("clears the tag when clear button is clicked", async () => {
    const onClear = vi.fn();
    const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
    renderCombobox({ appliedTag: "review", onClear });

    await user.click(screen.getByRole("button", { name: "Clear tag filter" }));
    expect(onClear).toHaveBeenCalledOnce();
  });

  it("fetches suggestions after 200ms debounce and shows dropdown", async () => {
    const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
    renderCombobox({ appliedTag: "" });

    const input = screen.getByRole("combobox", { name: "Filter by tag" });
    await user.type(input, "re");

    // Before 200ms: no request yet
    expect(busytokClient.promptsSuggestTags).not.toHaveBeenCalled();

    // After 200ms: request fires
    act(() => { vi.advanceTimersByTime(200); });
    await waitFor(() => {
      expect(busytokClient.promptsSuggestTags).toHaveBeenCalledWith(
        expect.objectContaining({ query: "re" }),
      );
    });

    await waitFor(() => {
      expect(screen.getByRole("option", { name: "review" })).toBeDefined();
    });
  });

  it("does not fetch suggestions within 200ms", async () => {
    const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
    renderCombobox({ appliedTag: "" });

    const input = screen.getByRole("combobox", { name: "Filter by tag" });
    await user.type(input, "r");
    act(() => { vi.advanceTimersByTime(100); });

    expect(busytokClient.promptsSuggestTags).not.toHaveBeenCalled();
  });

  it("only sends the final query after rapid typing", async () => {
    const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
    renderCombobox({ appliedTag: "" });

    const input = screen.getByRole("combobox", { name: "Filter by tag" });
    await user.type(input, "rev");

    act(() => { vi.advanceTimersByTime(200); });
    await waitFor(() => {
      expect(busytokClient.promptsSuggestTags).toHaveBeenCalledTimes(1);
    });
    expect(busytokClient.promptsSuggestTags).toHaveBeenCalledWith(
      expect.objectContaining({ query: "rev" }),
    );
  });

  it("applies tag when clicking a candidate", async () => {
    const onApplyTag = vi.fn();
    const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
    renderCombobox({ appliedTag: "", onApplyTag });

    const input = screen.getByRole("combobox", { name: "Filter by tag" });
    await user.type(input, "re");
    act(() => { vi.advanceTimersByTime(200); });

    await waitFor(() => {
      expect(screen.getByRole("option", { name: "review" })).toBeDefined();
    });

    await user.click(screen.getByRole("option", { name: "review" }));
    expect(onApplyTag).toHaveBeenCalledWith("review");
  });

  it("does not apply tag on Enter when no candidate is highlighted (default state)", async () => {
    const onApplyTag = vi.fn();
    const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
    renderCombobox({ appliedTag: "", onApplyTag });

    const input = screen.getByRole("combobox", { name: "Filter by tag" });
    await user.type(input, "re");
    act(() => { vi.advanceTimersByTime(200); });

    await waitFor(() => {
      expect(screen.getByRole("option", { name: "review" })).toBeDefined();
    });

    // No arrow key pressed → highlightIndex is null → Enter only closes dropdown
    await user.keyboard("{Enter}");
    expect(onApplyTag).not.toHaveBeenCalled();
  });

  it("applies tag on Enter after ArrowDown highlights a candidate", async () => {
    const onApplyTag = vi.fn();
    const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
    renderCombobox({ appliedTag: "", onApplyTag });

    const input = screen.getByRole("combobox", { name: "Filter by tag" });
    await user.type(input, "re");
    act(() => { vi.advanceTimersByTime(200); });

    await waitFor(() => {
      expect(screen.getByRole("option", { name: "refactor" })).toBeDefined();
    });

    // Down once highlights refactor(0)
    await user.keyboard("{ArrowDown}{Enter}");
    expect(onApplyTag).toHaveBeenCalledWith("refactor");
  });

  it("does not apply tag on Enter when no candidates exist", async () => {
    vi.mocked(busytokClient.promptsSuggestTags).mockResolvedValue({ tags: [] });
    const onApplyTag = vi.fn();
    const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
    renderCombobox({ appliedTag: "", onApplyTag });

    const input = screen.getByRole("combobox", { name: "Filter by tag" });
    await user.type(input, "zzz");
    act(() => { vi.advanceTimersByTime(200); });

    await waitFor(() => {
      expect(busytokClient.promptsSuggestTags).toHaveBeenCalled();
    });

    // No dropdown renders (no candidates → popover stays closed)
    expect(screen.queryByRole("option")).toBeNull();

    await user.keyboard("{Enter}");
    expect(onApplyTag).not.toHaveBeenCalled();
  });

  it("closes dropdown on Escape without clearing", async () => {
    const onClear = vi.fn();
    const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
    renderCombobox({ appliedTag: "", onClear });

    const input = screen.getByRole("combobox", { name: "Filter by tag" });
    await user.type(input, "re");
    act(() => { vi.advanceTimersByTime(200); });

    await waitFor(() => {
      expect(screen.getByRole("option", { name: "review" })).toBeDefined();
    });

    await user.keyboard("{Escape}");
    expect(screen.queryByRole("option")).toBeNull();
    expect(onClear).not.toHaveBeenCalled();
  });

  it("wraps around when navigating with arrow keys", async () => {
    const onApplyTag = vi.fn();
    const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
    renderCombobox({ appliedTag: "", onApplyTag });

    const input = screen.getByRole("combobox", { name: "Filter by tag" });
    await user.type(input, "re");
    act(() => { vi.advanceTimersByTime(200); });

    await waitFor(() => {
      expect(screen.getByRole("option", { name: "refactor" })).toBeDefined();
    });

    // ArrowDown from null wraps into list: null → 0 (refactor)
    // Then past last item wraps: 0 → 1 → 2 → 0 (refactor again)
    await user.keyboard("{ArrowDown}{ArrowDown}{ArrowDown}{ArrowDown}");
    await user.keyboard("{Enter}");
    expect(onApplyTag).toHaveBeenCalledWith("refactor");
  });

  it("opens dropdown on focus when input is empty and candidates arrive", async () => {
    const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
    renderCombobox({ appliedTag: "" });

    const input = screen.getByRole("combobox", { name: "Filter by tag" });
    await user.click(input);
    act(() => { vi.advanceTimersByTime(200); });

    // Query fires with empty string (fetch all)
    await waitFor(() => {
      expect(busytokClient.promptsSuggestTags).toHaveBeenCalledWith(
        expect.objectContaining({ query: "" }),
      );
    });

    // Dropdown opens once candidates are available
    await waitFor(() => {
      expect(screen.getByRole("option", { name: "refactor" })).toBeDefined();
    });
  });

  it("does not call onApplyTag while typing", async () => {
    const onApplyTag = vi.fn();
    const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
    renderCombobox({ appliedTag: "", onApplyTag });

    const input = screen.getByRole("combobox", { name: "Filter by tag" });
    await user.type(input, "rev");
    act(() => { vi.advanceTimersByTime(200); });
    expect(onApplyTag).not.toHaveBeenCalled();
  });

  it("allows Tab to reach clear button and Enter triggers onClear", async () => {
    const onClear = vi.fn();
    const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
    renderCombobox({ appliedTag: "review", onClear });

    // Tab from input to clear button
    const input = screen.getByRole("combobox", { name: "Filter by tag" });
    input.focus();
    await user.tab();

    const clearButton = screen.getByRole("button", { name: "Clear tag filter" });
    expect(clearButton).toBe(document.activeElement);

    await user.keyboard("{Enter}");
    expect(onClear).toHaveBeenCalledOnce();
  });

  it("renders its dropdown with shared .app-select__content and .app-select__item classes (canonical Combobox contract)", async () => {
    vi.mocked(busytokClient.promptsSuggestTags).mockResolvedValue({ tags: ["review", "refactor"] });
    const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
    renderCombobox({ appliedTag: "" });

    const input = screen.getByRole("combobox", { name: "Filter by tag" });
    await user.click(input);
    await user.type(input, "re");
    await vi.advanceTimersByTimeAsync(250);

    await waitFor(() => {
      const content = document.querySelector(".app-select__content");
      expect(content).not.toBeNull();
    });

    const items = document.querySelectorAll(".app-select__item");
    expect(items.length).toBe(2);
  });

  it("resets draftInput to appliedTag on blur when no candidates", async () => {
    vi.mocked(busytokClient.promptsSuggestTags).mockResolvedValue({ tags: [] });
    const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
    renderCombobox({ appliedTag: "review" });

    const input = screen.getByRole("combobox", { name: "Filter by tag" });
    await user.clear(input);
    await user.type(input, "zzz");
    act(() => { vi.advanceTimersByTime(200); });

    await waitFor(() => {
      expect(busytokClient.promptsSuggestTags).toHaveBeenCalled();
    });

    // No candidates → popover not rendered. Blur the input.
    await user.tab();
    expect((input as HTMLInputElement).value).toBe("review");
  });
});
