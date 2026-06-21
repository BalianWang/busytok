import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { SegmentedControl } from "./SegmentedControl";

afterEach(() => cleanup());

describe("SegmentedControl", () => {
  it("marks exactly one option active", () => {
    render(
      <SegmentedControl
        label="Range"
        value="day"
        options={[
          { value: "day", label: "Day" },
          { value: "week", label: "Week" },
        ]}
        onChange={vi.fn()}
      />,
    );

    expect(screen.getByRole("button", { name: "Day" }).className).toContain("is-active");
    expect(screen.getByRole("button", { name: "Week" }).className).not.toContain("is-active");
  });

  it("fires onChange with the selected value when an inactive option is clicked", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    render(
      <SegmentedControl
        label="Range"
        value="day"
        options={[
          { value: "day", label: "Day" },
          { value: "week", label: "Week" },
        ]}
        onChange={onChange}
      />,
    );

    await user.click(screen.getByRole("button", { name: "Week" }));

    expect(onChange).toHaveBeenCalledWith("week");
  });

  it("exposes the group label via aria-label on the container", () => {
    render(
      <SegmentedControl
        label="Range"
        value="day"
        options={[{ value: "day", label: "Day" }]}
        onChange={vi.fn()}
      />,
    );

    expect(screen.getByRole("group", { name: "Range" })).toBeDefined();
  });

  it("reflects aria-pressed state on the active option", () => {
    render(
      <SegmentedControl
        label="Range"
        value="week"
        options={[
          { value: "day", label: "Day" },
          { value: "week", label: "Week" },
        ]}
        onChange={vi.fn()}
      />,
    );

    expect(screen.getByRole("button", { name: "Day" }).getAttribute("aria-pressed")).toBe("false");
    expect(screen.getByRole("button", { name: "Week" }).getAttribute("aria-pressed")).toBe("true");
  });
});
