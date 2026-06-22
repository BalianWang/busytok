import { describe, expect, it, vi, beforeEach, afterEach } from "vitest";
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { ErrorBoundary } from "./components/ErrorBoundary";

// Mock localStorage for reporter (imported by ErrorBoundary)
const storage = (() => {
  let store: Record<string, string> = {};
  return {
    getItem: vi.fn((key: string) => (key in store ? store[key] : null)),
    setItem: vi.fn((key: string, value: string) => {
      store[key] = value;
    }),
    removeItem: vi.fn((key: string) => {
      delete store[key];
    }),
    clear: vi.fn(() => {
      store = {};
    }),
    get length() {
      return Object.keys(store).length;
    },
    key: vi.fn((index: number) => Object.keys(store)[index] ?? null),
  };
})();

Object.defineProperty(globalThis, "localStorage", {
  value: storage,
  writable: true,
  configurable: true,
});

function Bomb({ shouldExplode = true }: { shouldExplode?: boolean }) {
  if (shouldExplode) {
    throw new Error("BOOM — intentional render error for ErrorBoundary test");
  }
  return <p>safe</p>;
}

describe("ErrorBoundary", () => {
  let queryClient: QueryClient;

  beforeEach(() => {
    vi.spyOn(console, "error").mockImplementation(() => {});
    queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("renders children normally when no error occurs", () => {
    render(
      <QueryClientProvider client={queryClient}>
        <ErrorBoundary>
          <Bomb shouldExplode={false} />
        </ErrorBoundary>
      </QueryClientProvider>,
    );
    expect(screen.getByText("safe")).toBeDefined();
  });

  it("shows fallback UI when a child throws", () => {
    render(
      <QueryClientProvider client={queryClient}>
        <ErrorBoundary>
          <Bomb />
        </ErrorBoundary>
      </QueryClientProvider>,
    );
    expect(screen.getByText("Something went wrong")).toBeDefined();
  });

  it("offers a reload action when a child throws", async () => {
    const reload = vi.fn();
    Object.defineProperty(window, "location", {
      value: { reload },
      writable: true,
      configurable: true,
    });

    const view = render(
      <QueryClientProvider client={queryClient}>
        <ErrorBoundary>
          <Bomb />
        </ErrorBoundary>
      </QueryClientProvider>,
    );

    await userEvent.click(within(view.container).getByRole("button", { name: "Reload" }));

    expect(reload).toHaveBeenCalledOnce();
  });
});
