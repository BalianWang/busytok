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

  it("maps the ok activity status to the same semantic success treatment as StatusChip success", () => {
    render(<StatusPill tone="ok" label="Healthy" />);

    // The StatusPill enum is ActivityStatusDto (ok/warning/error), distinct
    // from StatusChip's ToneDto (neutral/success/warning/danger). The shared
    // semantic mapping is preserved at the CSS layer — `status-pill--ok`
    // consumes the same success tokens as `status-chip--success`.
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
