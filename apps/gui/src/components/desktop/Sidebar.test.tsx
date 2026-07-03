import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { Sidebar } from "./Sidebar";

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

describe("Sidebar", () => {
  it("renders all navigation items", () => {
    render(<Sidebar currentPage="overview" onNavigate={vi.fn()} />);

    // getByRole also asserts each label is on an interactive button, not just
    // present as text (matches the AppShell.test idiom).
    expect(screen.getByRole("button", { name: "Overview" })).toBeDefined();
    expect(screen.getByRole("button", { name: "Usage" })).toBeDefined();
    expect(screen.getByRole("button", { name: "Prompt Palette" })).toBeDefined();
    expect(screen.getByRole("button", { name: "Providers" })).toBeDefined();
    expect(screen.getByRole("button", { name: "Subagents" })).toBeDefined();
    expect(screen.getByRole("button", { name: "Settings" })).toBeDefined();
  });

  it("marks only the current page with aria-current", () => {
    const { container } = render(<Sidebar currentPage="usage" onNavigate={vi.fn()} />);

    expect(screen.getByRole("button", { name: "Usage" }).getAttribute("aria-current")).toBe("page");

    // No other item is marked current.
    const marked = container.querySelectorAll('.desktop-sidebar__item[aria-current="page"]');
    expect(marked.length).toBe(1);
  });

  // Regression: the sidebar previously rendered a decorative 58px empty
  // placeholder (`.desktop-sidebar__traffic-space`) above the nav — wasted
  // first-screen vertical space with no functional purpose (not a drag region,
  // not a traffic-light safety zone). The nav must now be the sidebar's first
  // child, and no such placeholder element may exist.
  it("does not render a decorative placeholder above the nav", () => {
    const { container } = render(<Sidebar currentPage="overview" onNavigate={vi.fn()} />);

    expect(container.querySelector(".desktop-sidebar__traffic-space")).toBeNull();

    const aside = container.querySelector(".desktop-sidebar");
    expect(aside).not.toBeNull();
    expect(aside?.firstElementChild?.tagName).toBe("NAV");
  });

  it("invokes onNavigate with the clicked page id", () => {
    const onNavigate = vi.fn();
    render(<Sidebar currentPage="overview" onNavigate={onNavigate} />);

    fireEvent.click(screen.getByRole("button", { name: "Settings" }));
    expect(onNavigate).toHaveBeenCalledWith("settings");
  });
});
