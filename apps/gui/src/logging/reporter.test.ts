// apps/gui/src/logging/reporter.test.ts

import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";

// Mock localStorage before importing reporter
const storage = (() => {
  let store: Record<string, string> = {};
  return {
    getItem: vi.fn((key: string) => (key in store ? store[key] : null)),
    setItem: vi.fn((key: string, value: string) => { store[key] = value; }),
    removeItem: vi.fn((key: string) => { delete store[key]; }),
    clear: vi.fn(() => { store = {}; }),
    get length() { return Object.keys(store).length; },
    key: vi.fn((index: number) => Object.keys(store)[index] ?? null),
    reset() {
      store = {};
      this.getItem.mockClear();
      this.setItem.mockClear();
      this.removeItem.mockClear();
      this.clear.mockClear();
      vi.mocked(this.key).mockClear();
    },
  };
})();

Object.defineProperty(globalThis, "localStorage", {
  value: storage, writable: true, configurable: true,
});

const { mockInvokeImpl } = vi.hoisted(() => {
  const fn = vi.fn();
  return { mockInvokeImpl: fn };
});

vi.mock("@tauri-apps/api/core", () => ({ invoke: mockInvokeImpl }));

let reporter: typeof import("./reporter");

beforeEach(async () => {
  storage.reset();
  mockInvokeImpl.mockReset();
  vi.resetModules();
  reporter = await import("./reporter");
  vi.useFakeTimers();
  vi.setSystemTime(new Date("2026-05-23T10:00:00Z"));
});

afterEach(() => {
  delete (window as Window & { busytokPanelBridge?: unknown }).busytokPanelBridge;
  vi.useRealTimers();
});

const BUFFER_KEY = "busytok_frontend_log_buffer";

/** Helper: call reportFrontendError with invoke mocked to reject, then await the catch handler. */
async function bufferError(entry: { event_code: string; message: string; details?: Record<string, unknown> }) {
  mockInvokeImpl.mockRejectedValueOnce(new Error("bridge down"));
  reporter.reportFrontendError(entry);
  // Buffer-first pattern: entry is written synchronously, then invoke is
  // attempted. On rejection, the catch handler is a no-op; entry stays buffered.
  await vi.runAllTimersAsync();
}

describe("getSessionId", () => {
  it("returns the same UUID across calls", () => {
    const id1 = reporter.getSessionId();
    const id2 = reporter.getSessionId();
    expect(id1).toBe(id2);
    expect(id1).toMatch(/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/);
  });
});

describe("reportFrontendError + dedup", () => {
  it("uses the panel bridge for frontend logs when running in the native panel", async () => {
    const panelInvoke = vi.fn().mockResolvedValue({ ok: true });
    (window as Window & {
      busytokPanelBridge?: {
        invoke: typeof panelInvoke;
        subscribe: () => () => void;
      };
    }).busytokPanelBridge = {
      invoke: panelInvoke,
      subscribe: () => () => {},
    };

    reporter.reportFrontendEvent({
      level: "INFO",
      event_code: "gui.prompt_palette.panel_probe",
      message: "panel probe",
    });

    await vi.runAllTimersAsync();

    expect(panelInvoke).toHaveBeenCalledWith(
      "log_frontend_event",
      expect.objectContaining({
        entry: expect.objectContaining({
          event_code: "gui.prompt_palette.panel_probe",
          message: "panel probe",
        }),
      }),
    );
    expect(mockInvokeImpl).not.toHaveBeenCalled();
    expect(reporter.hasBufferedLogs()).toBe(false);

    delete (window as Window & { busytokPanelBridge?: unknown }).busytokPanelBridge;
  });

  it("removes entry from buffer after invoke succeeds (no duplicate on flush)", async () => {
    // Buffer-first: entry is buffered synchronously, then invoke fires.
    // On success, entry is removed from buffer by ID.
    let resolveInvoke: (v: unknown) => void;
    const pendingInvoke = new Promise((r) => { resolveInvoke = r; });
    mockInvokeImpl.mockReturnValueOnce(pendingInvoke as Promise<unknown>);

    reporter.reportFrontendError({ event_code: "gui.unhandled_error", message: "test" });

    // Entry is in buffer immediately (synchronous)
    expect(reporter.hasBufferedLogs()).toBe(true);

    // Invoke succeeds → entry removed from buffer
    resolveInvoke!(undefined);
    await vi.runAllTimersAsync();
    expect(reporter.hasBufferedLogs()).toBe(false);

    // Subsequent heartbeat flush should be a no-op
    mockInvokeImpl.mockResolvedValueOnce({ written_count: 0, dropped_count: 0 });
    await reporter.flushBuffer();
    const flushCalls = mockInvokeImpl.mock.calls.filter((c: unknown[]) => c[0] === "flush_frontend_logs");
    expect(flushCalls.length).toBe(0);
  });

  it("buffers the error when invoke fails", async () => {
    await bufferError({
      event_code: "gui.unhandled_error",
      message: "test error",
      details: { stack: "Error: test\n  at foo.js:10" },
    });
    expect(reporter.hasBufferedLogs()).toBe(true);
  });

  it("deduplicates identical errors within 5 seconds", async () => {
    await bufferError({
      event_code: "gui.render_error", message: "boom",
      details: { stack: "Error: boom\n  at Bar.tsx:5" },
    });

    const count1 = JSON.parse(storage.getItem(BUFFER_KEY)!).entries.length;

    // Same error within 5s — should be suppressed
    vi.advanceTimersByTime(1000);
    await bufferError({
      event_code: "gui.render_error", message: "boom",
      details: { stack: "Error: boom\n  at Bar.tsx:5" },
    });

    const count2 = JSON.parse(storage.getItem(BUFFER_KEY)!).entries.length;
    expect(count2).toBe(count1);
  });

  it("allows identical errors after the 5s window", async () => {
    await bufferError({
      event_code: "gui.render_error", message: "boom",
      details: { stack: "Error: boom\n  at Bar.tsx:5" },
    });

    vi.advanceTimersByTime(6000);

    await bufferError({
      event_code: "gui.render_error", message: "boom",
      details: { stack: "Error: boom\n  at Bar.tsx:5" },
    });

    const entries = JSON.parse(storage.getItem(BUFFER_KEY)!).entries;
    expect(entries.length).toBe(2);
  });
});

describe("localStorage corruption recovery", () => {
  it("clears and recovers from corrupt JSON", () => {
    storage.setItem(BUFFER_KEY, "not valid json {{{");
    expect(reporter.hasBufferedLogs()).toBe(false);
  });

  it("clears and recovers from wrong version", () => {
    storage.setItem(BUFFER_KEY, JSON.stringify({ version: 99, entries: [] }));
    expect(reporter.hasBufferedLogs()).toBe(false);
  });

  it("filters out invalid entries on read", () => {
    storage.setItem(BUFFER_KEY, JSON.stringify({
      version: 1,
      entries: [
        "not an object",
        { no_event_code: true },
        { event_code: "gui.valid", message: "ok", level: "ERROR", ts: "x", session_id: "a" },
      ],
    }));
    expect(reporter.hasBufferedLogs()).toBe(true);
  });
});

describe("flushBuffer", () => {
  it("prevents concurrent flushes", async () => {
    // Buffer an entry via invoke rejection
    await bufferError({ event_code: "gui.error", message: "a" });

    // Set up a slow flush
    let resolveFlush: (v: unknown) => void;
    const slowFlush = new Promise((r) => { resolveFlush = r; });
    mockInvokeImpl.mockReturnValueOnce(slowFlush as Promise<unknown>);

    const flush1 = reporter.flushBuffer();
    const flush2 = reporter.flushBuffer(); // should be no-op

    resolveFlush!({ written_count: 1, dropped_count: 0 });
    await flush1;
    await flush2;

    expect(reporter.hasBufferedLogs()).toBe(false);
  });

  it("preserves buffer on flush failure", async () => {
    await bufferError({ event_code: "gui.error", message: "test" });
    expect(reporter.hasBufferedLogs()).toBe(true);

    mockInvokeImpl.mockRejectedValueOnce(new Error("still down"));
    await reporter.flushBuffer();

    expect(reporter.hasBufferedLogs()).toBe(true);
  });

  it("clears buffer on successful flush", async () => {
    await bufferError({ event_code: "gui.error", message: "test" });

    mockInvokeImpl.mockResolvedValueOnce({ written_count: 1, dropped_count: 0 });
    await reporter.flushBuffer();

    expect(reporter.hasBufferedLogs()).toBe(false);
  });

  it("drops invalid entries locally, does not re-send valid ones", async () => {
    // Buffer: 1 valid entry
    await bufferError({ event_code: "gui.error", message: "valid" });

    // Manually inject an invalid entry into the buffer (empty message)
    const raw = storage.getItem(BUFFER_KEY)!;
    const buf = JSON.parse(raw);
    buf.entries.push({
      ts: new Date().toISOString(),
      level: "ERROR",
      event_code: "gui.error",
      message: "",        // invalid — will be filtered before flush
      session_id: "test",
    });
    storage.setItem(BUFFER_KEY, JSON.stringify(buf));
    expect(reporter.hasBufferedLogs()).toBe(true);

    // Flush: 1 valid sent, 1 invalid filtered locally
    mockInvokeImpl.mockResolvedValueOnce({ written_count: 1, dropped_count: 0 });
    await reporter.flushBuffer();

    // Buffer should be empty — valid was sent, invalid was cleaned up
    expect(reporter.hasBufferedLogs()).toBe(false);

    // Second flush should be a no-op (nothing to re-send)
    const callsBefore = mockInvokeImpl.mock.calls.length;
    await reporter.flushBuffer();
    expect(mockInvokeImpl.mock.calls.length).toBe(callsBefore);
  });
});
