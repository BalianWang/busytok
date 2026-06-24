import { describe, it, expect } from "vitest";
import { render, screen } from "@testing-library/react";
import { SettingsStatus } from "./SettingsStatus";

describe("SettingsStatus", () => {
  it("renders the status label", () => {
    render(<SettingsStatus label="Running" />);
    expect(screen.getByText("Running")).toBeTruthy();
  });

  it("uses ok tone by default", () => {
    const { container } = render(<SettingsStatus label="OK" />);
    expect(container.querySelector(".settings-status--ok")).toBeTruthy();
  });

  it("applies warning tone", () => {
    const { container } = render(<SettingsStatus label="Degraded" tone="warning" />);
    expect(container.querySelector(".settings-status--warning")).toBeTruthy();
  });

  it("applies danger tone", () => {
    const { container } = render(<SettingsStatus label="Down" tone="danger" />);
    expect(container.querySelector(".settings-status--danger")).toBeTruthy();
  });

  it("applies muted tone", () => {
    const { container } = render(<SettingsStatus label="Unknown" tone="muted" />);
    expect(container.querySelector(".settings-status--muted")).toBeTruthy();
  });

  it("renders a status dot for non-muted tones", () => {
    const { container } = render(<SettingsStatus label="Active" tone="ok" />);
    expect(container.querySelector(".settings-status__dot")).toBeTruthy();
  });

  it("suppresses the dot in muted tone", () => {
    const { container } = render(<SettingsStatus label="Idle" tone="muted" />);
    expect(container.querySelector(".settings-status__dot")).toBeNull();
  });

  it("uses default size when no size prop", () => {
    const { container } = render(<SettingsStatus label="x" />);
    expect(container.querySelector(".settings-status--default")).toBeTruthy();
  });

  it("applies dense class for dense size", () => {
    const { container } = render(<SettingsStatus label="x" size="dense" />);
    expect(container.querySelector(".settings-status--dense")).toBeTruthy();
  });
});
