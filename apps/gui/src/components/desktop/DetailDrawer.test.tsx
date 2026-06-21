import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { DetailDrawer } from "./DetailDrawer";

describe("DetailDrawer", () => {
  afterEach(cleanup);

  it("renders content when open", () => {
    render(
      <DetailDrawer open title="Details" onClose={() => {}}>
        <p>Content</p>
      </DetailDrawer>,
    );
    expect(screen.getByText("Content")).toBeDefined();
  });

  it("does not render content when closed", () => {
    render(
      <DetailDrawer open={false} title="Details" onClose={() => {}}>
        <p>Content</p>
      </DetailDrawer>,
    );
    expect(screen.queryByText("Content")).toBeNull();
  });

  it("calls onClose when Escape is pressed", async () => {
    const onClose = vi.fn();
    render(
      <DetailDrawer open title="Details" onClose={onClose}>
        <p>Content</p>
      </DetailDrawer>,
    );
    await userEvent.keyboard("{Escape}");
    expect(onClose).toHaveBeenCalled();
  });

  it("renders the title", () => {
    render(
      <DetailDrawer open title="Activity Detail" onClose={() => {}}>
        <p>Content</p>
      </DetailDrawer>,
    );
    expect(screen.getByText("Activity Detail")).toBeDefined();
  });
});
