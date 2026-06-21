import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { DataPrivacyPanel } from "./DataPrivacyPanel";

afterEach(() => cleanup());

describe("DataPrivacyPanel", () => {
  it("renders the data and privacy heading", () => {
    render(<DataPrivacyPanel />);
    expect(screen.getByText("Data and privacy")).toBeDefined();
  });

  it("renders the local-first description", () => {
    render(<DataPrivacyPanel />);
    expect(screen.getByText(/local-first log audit tool/)).toBeDefined();
  });

  it("renders stored on this device section", () => {
    render(<DataPrivacyPanel />);
    expect(screen.getByText("Stored on this device")).toBeDefined();
  });

  it("renders usage history retention section", () => {
    render(<DataPrivacyPanel />);
    expect(screen.getByText("Usage history retention")).toBeDefined();
  });

  it("renders privacy stance section", () => {
    render(<DataPrivacyPanel />);
    expect(screen.getByText("Privacy stance")).toBeDefined();
  });

  it("mentions no cloud telemetry", () => {
    render(<DataPrivacyPanel />);
    expect(screen.getByText(/no cloud telemetry/)).toBeDefined();
  });

  it("mentions no network interception", () => {
    render(<DataPrivacyPanel />);
    expect(screen.getByText(/no network interception/)).toBeDefined();
  });

  it("has aria-label for accessibility", () => {
    render(<DataPrivacyPanel />);
    const section = document.querySelector('[aria-label="Data and privacy"]');
    expect(section).not.toBeNull();
  });
});
