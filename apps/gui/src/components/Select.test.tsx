import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { AppSelect, AppSelectItem } from "./Select";

afterEach(() => cleanup());

describe("AppSelect", () => {
  it("renders the select trigger with its accessible label", () => {
    render(
      <AppSelect value="a" onValueChange={vi.fn()} label="Sort">
        <AppSelectItem value="a">A</AppSelectItem>
      </AppSelect>,
    );

    expect(screen.getByRole("combobox", { name: "Sort" })).toBeDefined();
  });

  it("shows the visible field label separate from the trigger aria-label", () => {
    render(
      <AppSelect value="a" onValueChange={vi.fn()} label="Sort by">
        <AppSelectItem value="a">A</AppSelectItem>
      </AppSelect>,
    );

    // The visible label is rendered as a span (for sighted users), while the
    // trigger carries the accessible name. Both must be present.
    expect(screen.getByText("Sort by")).toBeDefined();
    expect(screen.getByRole("combobox", { name: "Sort by" })).toBeDefined();
  });

  it("displays the selected option's label inside the trigger", () => {
    render(
      <AppSelect value="a" onValueChange={vi.fn()} label="Sort">
        <AppSelectItem value="a">First</AppSelectItem>
        <AppSelectItem value="b">Second</AppSelectItem>
      </AppSelect>,
    );

    expect(screen.getByRole("combobox", { name: "Sort" }).textContent).toContain("First");
  });

  it("renders with default size when no size prop is given", () => {
    render(
      <AppSelect value="a" onValueChange={() => {}} label="Test">
        <AppSelectItem value="a">A</AppSelectItem>
      </AppSelect>,
    );
    const trigger = screen.getByRole("combobox");
    expect(trigger.classList.contains("app-select__trigger--default")).toBe(true);
  });

  it("applies dense class when size='dense'", () => {
    render(
      <AppSelect value="a" onValueChange={() => {}} label="Test" size="dense">
        <AppSelectItem value="a">A</AppSelectItem>
      </AppSelect>,
    );
    const trigger = screen.getByRole("combobox");
    expect(trigger.classList.contains("app-select__trigger--dense")).toBe(true);
  });
});
