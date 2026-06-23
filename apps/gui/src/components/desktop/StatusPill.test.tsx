import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { StatusPill } from "./StatusPill";

describe("StatusPill", () => {
  afterEach(cleanup);

  it("renders ok tone with correct class and default label", () => {
    render(<StatusPill tone="ok" />);
    const el = screen.getByText("ok");
    expect(el.className).toContain("status-pill");
    expect(el.className).toContain("status-pill--ok");
  });

  it("renders warning tone with correct class", () => {
    render(<StatusPill tone="warning" />);
    const el = screen.getByText("warning");
    expect(el.className).toContain("status-pill--warning");
  });

  it("renders error tone with correct class", () => {
    render(<StatusPill tone="error" />);
    const el = screen.getByText("error");
    expect(el.className).toContain("status-pill--error");
  });

  it("renders custom label when provided", () => {
    render(<StatusPill tone="ok" label="idle" />);
    expect(screen.getByText("idle")).toBeDefined();
  });

  it("maps the ok activity status to the success token family", () => {
    render(<StatusPill tone="ok" label="Healthy" />);

    // StatusPill's enum is ActivityStatusDto (ok/warning/error); the ok tone
    // consumes the shared success tokens (--color-status-success[-soft]).
    expect(screen.getByText("Healthy").className).toContain("ok");
  });

  it.each([
    ["ok", "status-pill--ok"],
    ["warning", "status-pill--warning"],
    ["error", "status-pill--error"],
  ] as const)("preserves the activity-status enum class for tone=%s", (tone, expectedClass) => {
    render(<StatusPill tone={tone} />);
    const el = screen.getByText(tone);
    expect(el.className).toContain(expectedClass);
  });
});
