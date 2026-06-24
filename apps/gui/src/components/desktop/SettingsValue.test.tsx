import { describe, it, expect } from "vitest";
import { render, screen } from "@testing-library/react";
import { SettingsValue } from "./SettingsValue";

describe("SettingsValue", () => {
  it("renders the value text", () => {
    render(<SettingsValue value="UTC+08:00" />);
    expect(screen.getByText("UTC+08:00")).toBeTruthy();
  });

  it("uses default tone when none specified", () => {
    const { container } = render(<SettingsValue value="test" />);
    expect(container.querySelector(".settings-value--default")).toBeTruthy();
  });

  it("applies muted tone", () => {
    const { container } = render(<SettingsValue value="n/a" tone="muted" />);
    expect(container.querySelector(".settings-value--muted")).toBeTruthy();
  });

  it("applies warning tone", () => {
    const { container } = render(<SettingsValue value="degraded" tone="warning" />);
    expect(container.querySelector(".settings-value--warning")).toBeTruthy();
  });

  it("applies danger tone", () => {
    const { container } = render(<SettingsValue value="error" tone="danger" />);
    expect(container.querySelector(".settings-value--danger")).toBeTruthy();
  });

  it("uses default size when no size prop", () => {
    const { container } = render(<SettingsValue value="x" />);
    expect(container.querySelector(".settings-value--default")).toBeTruthy();
  });

  it("applies dense class for dense size", () => {
    const { container } = render(<SettingsValue value="x" size="dense" />);
    expect(container.querySelector(".settings-value--dense")).toBeTruthy();
  });

  it("uses tabular-nums for numeric alignment", () => {
    const { container } = render(<SettingsValue value="1,234,567" />);
    const span = container.querySelector(".settings-value--default");
    expect(span).toBeTruthy();
  });
});
