import { describe, it, expect, vi, beforeEach } from "vitest";
import { renderHook, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { type ReactNode } from "react";

const fetchMock = vi.fn();
vi.stubGlobal("fetch", fetchMock);

import { useVersionHistory } from "./useVersionHistory";

function wrapper({ children }: { children: ReactNode }) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return <QueryClientProvider client={qc}>{children}</QueryClientProvider>;
}

beforeEach(() => { vi.clearAllMocks(); fetchMock.mockReset(); });

describe("useVersionHistory", () => {
  it("parses the versions array", async () => {
    fetchMock.mockResolvedValue({ ok: true, json: async () => ({ versions: [{ version: "0.1.0-rc.4", date: "d", notes: "n", manifest_url: "u" }] }) });
    const { result } = renderHook(() => useVersionHistory(), { wrapper });
    await waitFor(() => expect(result.current.data?.versions).toHaveLength(1));
    expect(result.current.data?.versions[0].version).toBe("0.1.0-rc.4");
  });

  it("tolerates a missing versions field", async () => {
    fetchMock.mockResolvedValue({ ok: true, json: async () => ({}) });
    const { result } = renderHook(() => useVersionHistory(), { wrapper });
    await waitFor(() => expect(result.current.data?.versions).toEqual([]));
  });

  it("surfaces fetch errors as isError", async () => {
    fetchMock.mockRejectedValue(new Error("net"));
    const { result } = renderHook(() => useVersionHistory(), { wrapper });
    await waitFor(() => expect(result.current.isError).toBe(true), { timeout: 3000 });
  });

  it("surfaces a non-ok HTTP response as isError", async () => {
    fetchMock.mockResolvedValue({ ok: false, status: 404, json: async () => ({}) });
    const { result } = renderHook(() => useVersionHistory(), { wrapper });
    await waitFor(() => expect(result.current.isError).toBe(true), { timeout: 3000 });
  });
});
