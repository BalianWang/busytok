import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { SettingsRow } from "./SettingsRow";

describe("SettingsRow", () => {
  afterEach(cleanup);

  it("renders label and control", () => {
    render(
      <SettingsRow
        label="Default range"
        control={<select aria-label="Default range"><option>week</option></select>}
      />,
    );
    expect(screen.getByText("Default range")).toBeDefined();
    expect(screen.getByRole("combobox", { name: /Default range/i })).toBeDefined();
  });

  it("renders description when provided", () => {
    render(
      <SettingsRow
        label="Budget"
        description="Set monthly consumption limits."
        control={<input aria-label="Budget target" type="number" />}
      />,
    );
    expect(screen.getByText("Budget")).toBeDefined();
    expect(screen.getByText("Set monthly consumption limits.")).toBeDefined();
    expect(screen.getByRole("spinbutton", { name: /Budget target/i })).toBeDefined();
  });

  it("does not render description element when omitted", () => {
    const { container } = render(
      <SettingsRow
        label="Default range"
        control={<select aria-label="Default range"><option>week</option></select>}
      />,
    );
    expect(screen.getByText("Default range")).toBeDefined();
    // The description <p> should not be in the DOM
    expect(container.querySelector(".settings-row p")).toBeNull();
  });

  it("renders inline error when provided", () => {
    render(
      <SettingsRow
        label="Timezone"
        error="Invalid timezone"
        control={<input aria-label="Timezone" />}
      />,
    );
    expect(screen.getByText("Invalid timezone")).toBeDefined();
  });

  it("does not render error element when error is null", () => {
    const { container } = render(
      <SettingsRow
        label="Timezone"
        control={<input aria-label="Timezone" />}
        error={null}
      />,
    );
    expect(container.querySelector(".settings-row__error")).toBeNull();
  });

  it("applies dangerous class when dangerous is true", () => {
    const { container } = render(
      <SettingsRow
        label="Reset"
        dangerous
        control={<button>Reset</button>}
      />,
    );
    expect(container.querySelector(".settings-row--dangerous")).toBeDefined();
  });

  it("does not apply dangerous class when dangerous is false/omitted", () => {
    const { container } = render(
      <SettingsRow
        label="Timezone"
        control={<input aria-label="Timezone" />}
      />,
    );
    expect(container.querySelector(".settings-row--dangerous")).toBeNull();
  });
});
