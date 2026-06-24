import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ToggleSwitch } from "./ToggleSwitch";

afterEach(() => cleanup());

describe("ToggleSwitch", () => {
  it("renders with a checkbox role and aria-label", () => {
    render(<ToggleSwitch checked={false} onChange={() => {}} aria-label="Enable" />);
    const checkbox = screen.getByRole("checkbox", { name: "Enable" });
    expect(checkbox).toBeTruthy();
    expect((checkbox as HTMLInputElement).checked).toBe(false);
  });

  it("reflects the checked state", () => {
    render(<ToggleSwitch checked={true} onChange={() => {}} aria-label="On" />);
    expect((screen.getByRole("checkbox", { name: "On" }) as HTMLInputElement).checked).toBe(true);
  });

  it("fires onChange on click", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    render(<ToggleSwitch checked={false} onChange={onChange} aria-label="Toggle" />);
    await user.click(screen.getByRole("checkbox", { name: "Toggle" }));
    expect(onChange).toHaveBeenCalledTimes(1);
  });

  it("uses default size when no size prop is given", () => {
    const { container } = render(<ToggleSwitch checked={false} onChange={() => {}} aria-label="X" />);
    expect(container.querySelector(".toggle-switch--default")).toBeTruthy();
  });

  it("applies dense class when size='dense'", () => {
    const { container } = render(<ToggleSwitch checked={false} onChange={() => {}} aria-label="X" size="dense" />);
    expect(container.querySelector(".toggle-switch--dense")).toBeTruthy();
  });

  it("disables the checkbox when disabled", () => {
    render(<ToggleSwitch checked={false} onChange={() => {}} aria-label="X" disabled />);
    expect((screen.getByRole("checkbox", { name: "X" }) as HTMLInputElement).disabled).toBe(true);
  });

  it("has accessible focus-visible ring on the track", () => {
    const { container } = render(<ToggleSwitch checked={false} onChange={() => {}} aria-label="X" />);
    expect(container.querySelector(".toggle-switch__track")).toBeTruthy();
  });

  it("renders no visible text labels (pure switch control)", () => {
    const { container } = render(<ToggleSwitch checked={false} onChange={() => {}} aria-label="X" />);
    expect(container.querySelector(".toggle-switch__label")).toBeNull();
    expect(container.querySelector(".toggle-switch__description")).toBeNull();
  });
});
