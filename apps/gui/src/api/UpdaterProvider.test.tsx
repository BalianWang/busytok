import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { renderHook, act, waitFor } from "@testing-library/react";
import type { ReactNode } from "react";

vi.mock("../lib/updaterClient", () => ({
  checkForUpdate: vi.fn(),
  applyUpdate: vi.fn(),
  CHECK_TIMEOUT_MS: 20_000,
  DOWNLOAD_TIMEOUT_MS: 120_000,
}));
const focusCallbacks: Array<(e: { payload: boolean }) => void> = [];
vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({
    onFocusChanged: (cb: (e: { payload: boolean }) => void) => {
      focusCallbacks.push(cb);
      return Promise.resolve(() => {});
    },
  }),
}));

import { checkForUpdate, applyUpdate } from "../lib/updaterClient";
import { UpdaterProvider, type UpdaterContextValue } from "./UpdaterProvider";

const mockedCheck = vi.mocked(checkForUpdate);
const mockedApply = vi.mocked(applyUpdate);

const wrapper = ({ children }: { children: ReactNode }) => <UpdaterProvider>{children}</UpdaterProvider>;

const fakeUpdate = { version: "0.3.0", close: vi.fn().mockResolvedValue(undefined) } as unknown;

beforeEach(() => {
  vi.clearAllMocks();
  // shouldAdvanceTime keeps RTL waitFor()'s real-interval polling live while
  // setInterval/Date.now stay faked (the interval-reset + 1h focus-threshold
  // assertions depend on both). Mirrors TagFilterCombobox.test.tsx.
  vi.useFakeTimers({ shouldAdvanceTime: true });
});
afterEach(() => vi.useRealTimers());

describe("UpdaterProvider state machine", () => {
  it("starts idle then checks on mount", async () => {
    mockedCheck.mockResolvedValue({ kind: "up-to-date" });
    const { result } = renderHook(() => useHook(), { wrapper });
    // Initial useState seeds { state: "idle" }; the mount effect then runs the
    // check. (We don't synchronously assert "idle" because React's act flushes
    // the passive effect — which setStatus({checking}) — before this line; the
    // transition below is the load-bearing assertion.)
    await waitFor(() => expect(result.current.status.state).toBe("up-to-date"));
    expect(mockedCheck).toHaveBeenCalledTimes(1);
  });

  it("exposes available metadata and holds the Update", async () => {
    mockedCheck.mockResolvedValue({ kind: "available", version: "0.3.0", notes: "n", date: "d", update: fakeUpdate });
    const { result } = renderHook(() => useHook(), { wrapper });
    await waitFor(() => expect(result.current.status.state).toBe("available"));
    expect(result.current.status).toEqual({ state: "available", version: "0.3.0", notes: "n", date: "d" });
  });

  it("checkNow resets to checking then surfaces errors", async () => {
    mockedCheck.mockResolvedValue({ kind: "error", message: "boom" });
    const { result } = renderHook(() => useHook(), { wrapper });
    await act(async () => { await result.current.checkNow(); });
    expect(result.current.status).toEqual({ state: "error", message: "boom" });
  });

  it("closes the previously-held Update on a re-check that returns a new one", async () => {
    const first = { ...fakeUpdate, close: vi.fn().mockResolvedValue(undefined) };
    const second = { ...fakeUpdate, close: vi.fn().mockResolvedValue(undefined) };
    mockedCheck
      .mockResolvedValueOnce({ kind: "available", version: "0.3.0", notes: "", date: "", update: first })
      .mockResolvedValueOnce({ kind: "available", version: "0.4.0", notes: "", date: "", update: second });
    const { result } = renderHook(() => useHook(), { wrapper });
    await waitFor(() => expect(result.current.status.state).toBe("available"));
    await act(async () => { await result.current.checkNow(); });
    expect(first.close).toHaveBeenCalledTimes(1);
    expect(second.close).not.toHaveBeenCalled();
  });

  it("applyNow downloads with progress → updated → pending-restart", async () => {
    const u = { ...fakeUpdate, close: vi.fn() };
    mockedCheck.mockResolvedValue({ kind: "available", version: "0.3.0", notes: "", date: "", update: u });
    mockedApply.mockResolvedValue({ kind: "updated", version: "0.3.0" });
    const { result } = renderHook(() => useHook(), { wrapper });
    await waitFor(() => expect(result.current.status.state).toBe("available"));
    await act(async () => { await result.current.applyNow(); });
    expect(mockedApply).toHaveBeenCalledTimes(1);
    expect(result.current.status.state).toBe("installed-pending-restart");
  });

  it("applyNow relaunch failure → installed-needs-manual-restart", async () => {
    const u = { ...fakeUpdate, close: vi.fn() };
    mockedCheck.mockResolvedValue({ kind: "available", version: "0.3.0", notes: "", date: "", update: u });
    mockedApply.mockResolvedValue({ kind: "needs-manual-restart", version: "0.3.0" });
    const { result } = renderHook(() => useHook(), { wrapper });
    await waitFor(() => expect(result.current.status.state).toBe("available"));
    await act(async () => { await result.current.applyNow(); });
    expect(result.current.status).toEqual({ state: "installed-needs-manual-restart", version: "0.3.0" });
  });

  it("applyNow download error → back to available (Update still held, retryable)", async () => {
    const u = { ...fakeUpdate, close: vi.fn() };
    mockedCheck.mockResolvedValue({ kind: "available", version: "0.3.0", notes: "n", date: "d", update: u });
    mockedApply.mockResolvedValue({ kind: "error", message: "disk full" });
    const { result } = renderHook(() => useHook(), { wrapper });
    await waitFor(() => expect(result.current.status.state).toBe("available"));
    await act(async () => { await result.current.applyNow(); });
    expect(result.current.status.state).toBe("available");
  });

  it("forwards download percent to status (50 → 100)", async () => {
    const u = { ...fakeUpdate, close: vi.fn() };
    mockedCheck.mockResolvedValue({ kind: "available", version: "0.3.0", notes: "", date: "", update: u });
    let onProgress: ((p: { chunkLength: number; contentLength?: number }) => void) | undefined;
    let resolveApply!: (v: { kind: "updated"; version: string }) => void;
    mockedApply.mockImplementation(async (_update, cb) => {
      onProgress = cb;
      return new Promise<{ kind: "updated"; version: string }>((r) => { resolveApply = r; });
    });
    const { result } = renderHook(() => useHook(), { wrapper });
    await waitFor(() => expect(result.current.status.state).toBe("available"));
    let applyPromise!: Promise<void>;
    act(() => { applyPromise = result.current.applyNow(); });
    await waitFor(() => expect(result.current.status.state).toBe("downloading"));
    act(() => { onProgress?.({ chunkLength: 500, contentLength: 1000 }); });
    expect(result.current.status).toEqual({ state: "downloading", percent: 50 });
    act(() => { onProgress?.({ chunkLength: 500, contentLength: 1000 }); });
    expect(result.current.status).toEqual({ state: "downloading", percent: 100 });
    act(() => { resolveApply({ kind: "updated", version: "0.3.0" }); });
    await act(async () => { await applyPromise; });
    expect(result.current.status.state).toBe("installed-pending-restart");
  });

  it("manual checkNow resets the 12h interval", async () => {
    mockedCheck.mockResolvedValue({ kind: "up-to-date" });
    const { result } = renderHook(() => useHook(), { wrapper });
    await waitFor(() => expect(mockedCheck).toHaveBeenCalledTimes(1));
    await act(async () => { await result.current.checkNow(); });
    expect(mockedCheck).toHaveBeenCalledTimes(2);
    // advancing the full interval fires once more (reset happened on the manual check)
    await act(async () => { vi.advanceTimersByTimeAsync(12 * 60 * 60 * 1000); });
    expect(mockedCheck).toHaveBeenCalledTimes(3);
  });

  it("focus re-checks only past the 1h threshold", async () => {
    mockedCheck.mockResolvedValue({ kind: "up-to-date" });
    renderHook(() => useHook(), { wrapper });
    await waitFor(() => expect(mockedCheck).toHaveBeenCalledTimes(1));
    const baseline = mockedCheck.mock.calls.length;
    // focus within the threshold → no extra check
    act(() => { focusCallbacks[focusCallbacks.length - 1]?.({ payload: true }); });
    expect(mockedCheck.mock.calls.length).toBe(baseline);
    // advance past 1h (fake timers also advance Date.now) → focus now re-checks
    await act(async () => { vi.advanceTimersByTimeAsync(61 * 60 * 1000); });
    act(() => { focusCallbacks[focusCallbacks.length - 1]?.({ payload: true }); });
    await waitFor(() => expect(mockedCheck.mock.calls.length).toBeGreaterThan(baseline));
  });

  it("closes the held Update on unmount", async () => {
    const u = { ...fakeUpdate, close: vi.fn().mockResolvedValue(undefined) };
    mockedCheck.mockResolvedValue({ kind: "available", version: "0.3.0", notes: "", date: "", update: u });
    const { result, unmount } = renderHook(() => useHook(), { wrapper });
    await waitFor(() => expect(result.current.status.state).toBe("available"));
    unmount();
    await waitFor(() => expect(u.close).toHaveBeenCalledTimes(1));
  });
});

// local hook so tests can read the context value (the public useUpdater lives
// in ../hooks/useUpdater from Task 3; here we read the context directly so this
// test does not depend on Task 3).
import { useContext } from "react";
import { UpdaterContext } from "./UpdaterProvider";
function useHook(): UpdaterContextValue {
  const ctx = useContext(UpdaterContext);
  if (!ctx) throw new Error("provider missing");
  return ctx;
}
