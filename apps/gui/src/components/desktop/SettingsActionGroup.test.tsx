import { describe, it, expect, afterEach } from "vitest";
import { render, screen, cleanup } from "@testing-library/react";
import { SettingsActionGroup } from "./SettingsActionGroup";
import { SettingsValue } from "./SettingsValue";
import { SettingsStatus } from "./SettingsStatus";

afterEach(() => cleanup());

describe("SettingsActionGroup", () => {
  it("renders its children", () => {
    render(
      <SettingsActionGroup>
        <span>child content</span>
      </SettingsActionGroup>,
    );
    expect(screen.getByText("child content")).toBeTruthy();
  });

  it("defaults to column direction", () => {
    const { container } = render(
      <SettingsActionGroup>
        <SettingsValue value="v1" />
        <button type="button">Retry</button>
      </SettingsActionGroup>,
    );
    expect(container.querySelector(".settings-action-group--col")).toBeTruthy();
  });

  it("accepts row direction", () => {
    const { container } = render(
      <SettingsActionGroup direction="row">
        <SettingsStatus label="OK" />
        <button type="button">Action</button>
      </SettingsActionGroup>,
    );
    expect(container.querySelector(".settings-action-group--row")).toBeTruthy();
  });

  it("composes a value + button layout", () => {
    render(
      <SettingsActionGroup>
        <SettingsValue value="Unavailable" tone="muted" />
        <button type="button">Retry</button>
      </SettingsActionGroup>,
    );
    expect(screen.getByText("Unavailable")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Retry" })).toBeTruthy();
  });
});
