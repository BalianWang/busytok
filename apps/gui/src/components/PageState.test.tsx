import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { PageState } from "./PageState";

afterEach(() => cleanup());

describe("PageState", () => {
  it("renders loading, empty, error, unavailable, and degraded shared states", () => {
    const { rerender } = render(
      <PageState
        kind="loading"
        title="Loading dashboard"
        message="Fetching the latest usage."
      />,
    );
    expect(screen.getByText(/loading dashboard/i)).toBeDefined();

    rerender(
      <PageState
        kind="empty"
        title="No activity yet"
        message="Connect an app to get started."
      />,
    );
    expect(screen.getByText(/no activity yet/i)).toBeDefined();

    rerender(
      <PageState
        kind="error"
        title="Could not load dashboard"
        message="Try again."
      />,
    );
    expect(screen.getByText(/could not load dashboard/i)).toBeDefined();

    rerender(
      <PageState
        kind="unavailable"
        title="Recovery tools unavailable"
        message="The service is still starting."
      />,
    );
    expect(screen.getByText(/recovery tools unavailable/i)).toBeDefined();

    rerender(
      <PageState
        kind="degraded"
        title="Partial data"
        message="Some metrics are still computing."
      />,
    );
    expect(screen.getByText(/partial data/i)).toBeDefined();
    expect(screen.getByText(/degraded/i)).toBeDefined();
  });

  it("renders optional action button when actionLabel and onAction are provided", () => {
    const onAction = vi.fn();
    render(
      <PageState
        kind="error"
        title="Could not load data"
        message="Something went wrong."
        actionLabel="Retry"
        onAction={onAction}
      />,
    );

    const button = screen.getByRole("button", { name: "Retry" });
    expect(button).toBeDefined();
  });

  it("fires onAction callback when action button is clicked", async () => {
    const user = userEvent.setup();
    const onAction = vi.fn();
    render(
      <PageState
        kind="error"
        title="Could not load data"
        message="Something went wrong."
        actionLabel="Retry"
        onAction={onAction}
      />,
    );

    await user.click(screen.getByRole("button", { name: "Retry" }));
    expect(onAction).toHaveBeenCalledOnce();
  });

  it("renders diagnostics count for degraded state", () => {
    render(
      <PageState
        kind="degraded"
        title="Partial data"
        message="Some metrics are still computing."
        diagnosticsCount={3}
      />,
    );
    expect(screen.getByText("3 diagnostics")).toBeDefined();
  });

  it("does not render diagnostics count when zero", () => {
    render(
      <PageState
        kind="degraded"
        title="Partial data"
        message="Some metrics are still computing."
        diagnosticsCount={0}
      />,
    );
    expect(screen.queryByText(/diagnostic/)).toBeNull();
  });

  it("does not render action button when actionLabel is omitted", () => {
    render(
      <PageState
        kind="empty"
        title="No data yet"
        message="Check back later."
      />,
    );
    expect(screen.queryByRole("button")).toBeNull();
  });
});
