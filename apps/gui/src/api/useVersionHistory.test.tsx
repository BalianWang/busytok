import { describe, it, expect, vi, beforeEach } from "vitest";
import { renderHook, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { type ReactNode } from "react";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

import { invoke } from "@tauri-apps/api/core";
import { useVersionHistory } from "./useVersionHistory";

const mockedInvoke = vi.mocked(invoke);

function wrapper({ children }: { children: ReactNode }) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return <QueryClientProvider client={qc}>{children}</QueryClientProvider>;
}

beforeEach(() => { vi.clearAllMocks(); });

describe("useVersionHistory", () => {
  it("returns the versions array surfaced by the Rust command", async () => {
    const entries = [{ version: "v0.1.0", date: "d", notes: "n", manifest_url: "u" }];
    mockedInvoke.mockResolvedValue(entries);
    const { result } = renderHook(() => useVersionHistory(), { wrapper });
    await waitFor(() => expect(result.current.data).toHaveLength(1));
    expect(result.current.data?.[0].version).toBe("v0.1.0");
  });

  it("surfaces invoke errors as isError", async () => {
    mockedInvoke.mockRejectedValue(new Error("versions.json request failed"));
    const { result } = renderHook(() => useVersionHistory(), { wrapper });
    await waitFor(() => expect(result.current.isError).toBe(true), { timeout: 3000 });
  });

  it("does not flag isError when the Rust command resolves", async () => {
    mockedInvoke.mockResolvedValue([]);
    const { result } = renderHook(() => useVersionHistory(), { wrapper });
    await waitFor(() => expect(result.current.data).toBeDefined());
    expect(result.current.isError).toBe(false);
  });
});
