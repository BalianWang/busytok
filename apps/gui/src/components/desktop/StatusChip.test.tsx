import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { StatusChip } from "./StatusChip";
import type { StatusChipDto, StatusActionDto } from "@busytok/protocol-types";

globalThis.ResizeObserver = class ResizeObserver {
  observe() {}
  unobserve() {}
  disconnect() {}
};

afterEach(() => cleanup());

function chip(overrides: Partial<StatusChipDto> = {}): StatusChipDto {
  return {
    id: "scan",
    label: "Scan ready",
    tone: "success",
    detail: null,
    action: null,
    ...overrides,
  };
}

describe("StatusChip", () => {
  it("renders a static chip when no detail or action is present", () => {
    render(<StatusChip model={chip()} />);

    expect(screen.getByLabelText("Scan ready").tagName).toBe("SPAN");
    expect(screen.getByText("Scan ready")).toBeDefined();
  });

  it.each<[StatusActionDto, string, string]>([
    ["open_activity", "View Activity", "usage"],
    ["open_settings", "Open Settings", "settings"],
  ])("navigates for %s actions", async (action, buttonLabel, targetPage) => {
    const user = userEvent.setup();
    const onAction = vi.fn();

    render(
      <StatusChip
        model={chip({
          detail: "Open the related page for more context.",
          action,
        })}
        onAction={onAction}
      />,
    );

    await user.click(screen.getByRole("button", { name: "Scan ready" }));
    expect(await screen.findByText("Open the related page for more context.")).toBeDefined();

    await user.click(screen.getByRole("button", { name: buttonLabel }));
    expect(onAction).toHaveBeenCalledWith(targetPage);
  });

  it("omits the action button when the shell did not provide a handler", async () => {
    const user = userEvent.setup();

    render(
      <StatusChip
        model={chip({
          detail: "Background service is warming up.",
          action: "open_settings",
        })}
      />,
    );

    await user.click(screen.getByRole("button", { name: "Scan ready" }));
    expect(await screen.findByText("Background service is warming up.")).toBeDefined();
    expect(screen.queryByRole("button", { name: "Open Settings" })).toBeNull();
  });

  it("ignores unknown action values defensively", async () => {
    const user = userEvent.setup();

    render(
      <StatusChip
        model={chip({
          detail: "Unknown action.",
          action: "unknown_action" as StatusActionDto,
        })}
        onAction={vi.fn()}
      />,
    );

    await user.click(screen.getByRole("button", { name: "Scan ready" }));
    expect(await screen.findByText("Unknown action.")).toBeDefined();
    expect(screen.queryByText(/View|Open/)).toBeNull();
  });

  it.each([
    ["neutral"],
    ["success"],
    ["warning"],
    ["danger"],
  ] as const)("emits the semantic visual-role class for tone=%s", (tone) => {
    render(<StatusChip model={chip({ label: `Tone ${tone}`, tone })} />);

    expect(screen.getByLabelText(`Tone ${tone}`).className).toContain(`status-chip--${tone}`);
  });

  it("maps success tone to the shared success treatment so CSS can swap internals freely", () => {
    render(<StatusChip model={chip({ label: "Healthy", tone: "success" })} />);

    // The class is the semantic contract. Internal colors can migrate via CSS
    // without forcing page-local enums to change.
    expect(screen.getByLabelText("Healthy").className).toContain("success");
  });
});
