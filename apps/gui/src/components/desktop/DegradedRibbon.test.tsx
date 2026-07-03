import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { DegradedRibbon } from "./DegradedRibbon";

afterEach(() => cleanup());

describe("DegradedRibbon", () => {
  it("renders nothing when show is false", () => {
    render(<DegradedRibbon show={false} reason={null} isStale={false} />);
    expect(document.querySelector(".degraded-ribbon")).toBeNull();
  });

  it("renders the provided reason when show is true", () => {
    render(<DegradedRibbon show={true} reason="Partial outage" isStale={false} />);
    expect(screen.getByText("Partial outage")).toBeDefined();
    expect(document.querySelector(".degraded-ribbon")).not.toBeNull();
  });

  it("shows stale message when reason is null and isStale is true", () => {
    render(<DegradedRibbon show={true} reason={null} isStale={true} />);
    expect(screen.getByText(/stale data/i)).toBeDefined();
  });

  it("shows approximate message when reason is null and isStale is false", () => {
    render(<DegradedRibbon show={true} reason={null} isStale={false} />);
    expect(screen.getByText(/approximate/i)).toBeDefined();
  });

  it("prefers reason over isStale-derived message", () => {
    render(<DegradedRibbon show={true} reason="Custom reason" isStale={true} />);
    expect(screen.getByText("Custom reason")).toBeDefined();
    expect(screen.queryByText(/stale data/i)).toBeNull();
  });
});
