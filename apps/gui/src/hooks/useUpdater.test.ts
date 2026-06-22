import { describe, it, expect, vi, beforeEach } from "vitest";
import { renderHook, act, waitFor } from "@testing-library/react";

vi.mock("../lib/updaterClient", () => ({
  checkAndApplyUpdate: vi.fn(),
}));

import { checkAndApplyUpdate } from "../lib/updaterClient";
import { useUpdater } from "./useUpdater";

const mocked = vi.mocked(checkAndApplyUpdate);

beforeEach(() => {
  vi.clearAllMocks();
});

describe("useUpdater", () => {
  it("starts idle and does NOT auto-fire", () => {
    const { result } = renderHook(() => useUpdater());
    expect(result.current.status.state).toBe("idle");
    expect(mocked).not.toHaveBeenCalled();
  });

  it("checkNow fires check and exposes result", async () => {
    mocked.mockResolvedValue({ kind: "updated", version: "0.2.1" });
    const { result } = renderHook(() => useUpdater());
    await act(async () => { await result.current.checkNow(); });
    expect(mocked).toHaveBeenCalledTimes(1);
    expect(result.current.status).toEqual({
      state: "done", result: { kind: "updated", version: "0.2.1" },
    });
  });

  it("transitions through checking state", async () => {
    let resolve: (v: { kind: "up-to-date" }) => void = () => {};
    mocked.mockImplementation(() => new Promise((r) => { resolve = r; }));
    const { result } = renderHook(() => useUpdater());
    act(() => {
      void result.current.checkNow();
    });
    await waitFor(() => {
      expect(result.current.status.state).toBe("checking");
    });
    resolve({ kind: "up-to-date" });
    await waitFor(() => {
      expect(result.current.status.state).toBe("done");
    });
    expect(result.current.status).toEqual({ state: "done", result: { kind: "up-to-date" } });
  });

  it("exposes errors in status", async () => {
    mocked.mockResolvedValue({ kind: "error", message: "boom" });
    const { result } = renderHook(() => useUpdater());
    await act(async () => { await result.current.checkNow(); });
    expect(result.current.status).toEqual({ state: "done", result: { kind: "error", message: "boom" } });
  });
});
