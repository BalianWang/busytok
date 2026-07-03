import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, fireEvent, cleanup } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { UsagePage } from "./UsagePage";

vi.mock("./ActivityPage", () => ({ ActivityPage: () => <div>Activity Content</div> }));
vi.mock("./ProjectsPage", () => ({ ProjectsPage: () => <div>Projects Content</div> }));
vi.mock("./ModelsPage", () => ({ ModelsPage: () => <div>Models Content</div> }));
vi.mock("./SessionsPage", () => ({ SessionsPage: () => <div>Sessions Content</div> }));

function renderUsagePage() {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <UsagePage />
    </QueryClientProvider>,
  );
}

describe("UsagePage", () => {
  afterEach(() => {
    cleanup();
  });

  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("renders four tabs", () => {
    renderUsagePage();
    expect(screen.getByRole("tab", { name: "Activity" })).toBeDefined();
    expect(screen.getByRole("tab", { name: "Projects" })).toBeDefined();
    expect(screen.getByRole("tab", { name: "Models" })).toBeDefined();
    expect(screen.getByRole("tab", { name: "Sessions" })).toBeDefined();
  });

  it("renders Activity tab active by default", () => {
    renderUsagePage();
    const activityTab = screen.getByRole("tab", { name: "Activity" });
    const projectsTab = screen.getByRole("tab", { name: "Projects" });
    expect(activityTab.getAttribute("aria-selected")).toBe("true");
    expect(projectsTab.getAttribute("aria-selected")).toBe("false");
  });

  it("shows Activity content by default", () => {
    renderUsagePage();
    expect(screen.getByText("Activity Content")).toBeDefined();
    expect(screen.queryByText("Projects Content")).toBeNull();
  });

  it("switches content when clicking a tab", () => {
    renderUsagePage();
    fireEvent.click(screen.getByRole("tab", { name: "Projects" }));
    expect(screen.getByText("Projects Content")).toBeDefined();
    expect(screen.queryByText("Activity Content")).toBeNull();
  });

  it("renders underline indicator for active tab", () => {
    renderUsagePage();
    const indicator = document.querySelector(".usage-tabs__indicator");
    expect(indicator).toBeDefined();
    // jsdom does not compute CSS layout, so offsetWidth is 0.
    // Verify presence + that the style attribute is set.
    expect(indicator!.getAttribute("style")).toContain("width");
  });

  // ── handleKeyDown: keyboard navigation between tabs ────────────────

  it("moves to the next tab on ArrowRight", () => {
    renderUsagePage();
    const activityTab = screen.getByRole("tab", { name: "Activity" });
    fireEvent.keyDown(activityTab, { key: "ArrowRight" });
    // Activity (index 0) → Projects (index 1)
    expect(screen.getByText("Projects Content")).toBeDefined();
    expect(screen.queryByText("Activity Content")).toBeNull();
    expect(ProjectsTabIsActive()).toBe(true);
  });

  it("wraps forward from the last tab to the first on ArrowRight", () => {
    renderUsagePage();
    // Move to Sessions (last tab) first.
    fireEvent.click(screen.getByRole("tab", { name: "Sessions" }));
    expect(screen.getByText("Sessions Content")).toBeDefined();

    const sessionsTab = screen.getByRole("tab", { name: "Sessions" });
    fireEvent.keyDown(sessionsTab, { key: "ArrowRight" });
    // Sessions (index 3) → Activity (index 0) via modulo wrap.
    expect(screen.getByText("Activity Content")).toBeDefined();
    expect(screen.queryByText("Sessions Content")).toBeNull();
  });

  it("moves to the previous tab on ArrowLeft", () => {
    renderUsagePage();
    // Move to Models (index 2) first.
    fireEvent.click(screen.getByRole("tab", { name: "Models" }));

    const modelsTab = screen.getByRole("tab", { name: "Models" });
    fireEvent.keyDown(modelsTab, { key: "ArrowLeft" });
    // Models (index 2) → Projects (index 1).
    expect(screen.getByText("Projects Content")).toBeDefined();
    expect(screen.queryByText("Models Content")).toBeNull();
  });

  it("wraps backward from the first tab to the last on ArrowLeft", () => {
    renderUsagePage();
    const activityTab = screen.getByRole("tab", { name: "Activity" });
    fireEvent.keyDown(activityTab, { key: "ArrowLeft" });
    // Activity (index 0) → Sessions (index 3) via backward wrap.
    expect(screen.getByText("Sessions Content")).toBeDefined();
    expect(screen.queryByText("Activity Content")).toBeNull();
  });

  it("does not change tabs on a non-navigation key", () => {
    renderUsagePage();
    const activityTab = screen.getByRole("tab", { name: "Activity" });
    fireEvent.keyDown(activityTab, { key: "Enter" });
    // Activity stays active.
    expect(screen.getByText("Activity Content")).toBeDefined();
  });
});

function ProjectsTabIsActive(): boolean {
  const projectsTab = screen.getByRole("tab", { name: "Projects" });
  return projectsTab.getAttribute("aria-selected") === "true";
}
