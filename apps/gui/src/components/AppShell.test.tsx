import { render, screen, cleanup } from "@testing-library/react";
import { useMemo } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { AppShell, type DesktopPage } from "./AppShell";
import { PageToolbarProvider, useRegisterPageToolbar } from "./desktop/PageToolbarContext";

function Wrapper({ children }: { children: React.ReactNode }) {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return (
    <QueryClientProvider client={queryClient}>
      <PageToolbarProvider>{children}</PageToolbarProvider>
    </QueryClientProvider>
  );
}

function ToolbarRegistrant() {
  const toolbar = useMemo(
    () => (
      <button type="button" aria-label="Refresh data">
        Refresh
      </button>
    ),
    [],
  );
  useRegisterPageToolbar(toolbar);
  return <p>Toolbar content</p>;
}

// Default shell status — ready_exact with no chips
vi.mock("../api/useBusytokData", () => ({
  useShellStatus: () => ({
    data: {
      generated_at_ms: Date.now(),
      status_chips: [],
      readiness: "ready_exact" as const,
      latest_event_seq: 123,
      writer_queue_depth: null,
      aggregate_lag_ms: null,
      subscription_bridge_connectivity: "connected",
    },
    isLoading: false,
    isError: false,
  }),
}));

vi.mock("../api/useEventSubscription", () => ({
  useEventSubscription: () => ({
    connectionStatus: "connected" as const,
  }),
}));

// UpdateBadgeButton renders null when up-to-date/idle, so stub it to assert its
// slot lives inside the right-aligned .desktop-titlebar__actions group.
vi.mock("./desktop/UpdateBadgeButton", () => ({
  UpdateBadgeButton: () => <div data-testid="update-badge" />,
}));

describe("AppShell", () => {
  afterEach(() => cleanup());

  it("renders desktop shell container", () => {
    render(
      <Wrapper>
        <AppShell currentPage="overview" onNavigate={() => {}}>
          <p>Content</p>
        </AppShell>
      </Wrapper>,
    );
    expect(document.querySelector(".desktop-shell")).not.toBeNull();
  });

  it("renders children inside workspace", () => {
    render(
      <Wrapper>
        <AppShell currentPage="overview" onNavigate={() => {}}>
          <p>Dashboard content</p>
        </AppShell>
      </Wrapper>,
    );
    expect(screen.getByText("Dashboard content")).toBeDefined();
  });

  it("renders the current sidebar items", () => {
    render(
      <Wrapper>
        <AppShell currentPage="overview" onNavigate={() => {}}>
          <p>Content</p>
        </AppShell>
      </Wrapper>,
    );
    const allButtons = document.querySelectorAll(".desktop-sidebar__item");
    expect(allButtons.length).toBe(6);
  });

  it("calls onNavigate when named sidebar buttons are clicked", async () => {
    let navigatedTo: DesktopPage | undefined;
    render(
      <Wrapper>
        <AppShell currentPage="overview" onNavigate={(page) => { navigatedTo = page; }}>
          <p>Content</p>
        </AppShell>
      </Wrapper>,
    );
    screen.getByRole("button", { name: "Prompt Palette" }).click();
    expect(navigatedTo).toBe("prompt_palette");
    screen.getByRole("button", { name: "Settings" }).click();
    expect(navigatedTo).toBe("settings");
  });

  it("does not contain proxy or tracking language", () => {
    render(
      <Wrapper>
        <AppShell currentPage="overview" onNavigate={() => {}}>
          <p>Content</p>
        </AppShell>
      </Wrapper>,
    );
    expect(screen.queryByText(/proxy/i)).toBeNull();
    expect(screen.queryByText(/tracking/i)).toBeNull();
    expect(screen.queryByText(/credential/i)).toBeNull();
  });

  it("renders Overview, Usage, Prompt Palette, and Settings buttons", () => {
    render(
      <Wrapper>
        <AppShell currentPage="overview" onNavigate={() => {}}>
          <p>Content</p>
        </AppShell>
      </Wrapper>,
    );
    expect(screen.getByRole("button", { name: "Overview" })).toBeDefined();
    expect(screen.getByRole("button", { name: "Usage" })).toBeDefined();
    expect(screen.getByRole("button", { name: "Prompt Palette" })).toBeDefined();
    expect(screen.getByRole("button", { name: "Settings" })).toBeDefined();
  });

  it("does not render a Ready badge when ready_exact", () => {
    render(
      <Wrapper>
        <AppShell currentPage="overview" onNavigate={() => {}}>
          <p>Content</p>
        </AppShell>
      </Wrapper>,
    );
    expect(screen.queryByText("Ready")).toBeNull();
  });

  it("does not show queue depth when the default shell status omits it", () => {
    render(
      <Wrapper>
        <AppShell currentPage="overview" onNavigate={() => {}}>
          <p>Content</p>
        </AppShell>
      </Wrapper>,
    );
    expect(screen.queryByText(/Q:/)).toBeNull();
  });

  it("does not show progress banner when ready_exact", () => {
    render(
      <Wrapper>
        <AppShell currentPage="overview" onNavigate={() => {}}>
          <p>Content</p>
        </AppShell>
      </Wrapper>,
    );
    expect(document.querySelector(".desktop-progress-banner")).toBeNull();
  });

  it("renders page-registered toolbar content inside the titlebar", () => {
    render(
      <Wrapper>
        <AppShell currentPage="overview" onNavigate={() => {}}>
          <ToolbarRegistrant />
        </AppShell>
      </Wrapper>,
    );

    const toolbarButton = screen.getByRole("button", { name: "Refresh data" });
    expect(toolbarButton.closest(".desktop-titlebar__actions")).not.toBeNull();
  });

  it("wraps the page toolbar in a right-aligned .desktop-titlebar__actions group", () => {
    render(
      <Wrapper>
        <AppShell currentPage="overview" onNavigate={() => {}}>
          <ToolbarRegistrant />
        </AppShell>
      </Wrapper>,
    );

    const actions = document.querySelector(".desktop-titlebar__actions");
    expect(actions).not.toBeNull();
    expect(actions?.querySelector('[data-testid="update-badge"]')).not.toBeNull();
    // Toolbar content sits inside the same actions cluster.
    expect(screen.getByRole("button", { name: "Refresh data" }).closest(".desktop-titlebar__actions")).not.toBeNull();
  });

  it("does not render a vestigial traffic-light gutter before the status chip", () => {
    // The main window uses native decorations, so macOS traffic lights sit in
    // the native bar above the webview — nothing overlays this titlebar. Any
    // reserved gutter is dead space pushing the status chip off the content
    // rhythm. The status group must be the titlebar's first child.
    render(
      <Wrapper>
        <AppShell currentPage="overview" onNavigate={() => {}}>
          <p>Content</p>
        </AppShell>
      </Wrapper>,
    );
    expect(document.querySelector(".desktop-titlebar__gutter")).toBeNull();
    const titlebar = document.querySelector(".desktop-titlebar");
    expect(titlebar?.firstElementChild?.classList.contains("desktop-titlebar__status")).toBe(true);
  });
});
