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
});
