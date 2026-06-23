import { describe, it, expect, vi, beforeEach } from "vitest";

vi.mock("@tauri-apps/plugin-updater", () => ({ check: vi.fn() }));
vi.mock("@tauri-apps/plugin-process", () => ({ relaunch: vi.fn() }));

import { check } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";
import { checkForUpdate, applyUpdate, CHECK_TIMEOUT_MS, DOWNLOAD_TIMEOUT_MS } from "./updaterClient";

const mockedCheck = vi.mocked(check);
const mockedRelaunch = vi.mocked(relaunch);

function fakeUpdate(overrides: Partial<{ version: string; body: string; date: string; downloadAndInstall: ReturnType<typeof vi.fn>; close: ReturnType<typeof vi.fn> }> = {}) {
  return {
    version: "0.2.1",
    body: "release notes",
    date: "2026-06-23",
    downloadAndInstall: vi.fn().mockResolvedValue(undefined),
    close: vi.fn().mockResolvedValue(undefined),
    ...overrides,
  };
}

beforeEach(() => vi.clearAllMocks());

describe("checkForUpdate", () => {
  it("returns up-to-date when check() resolves null", async () => {
    mockedCheck.mockResolvedValue(null);
    expect(await checkForUpdate()).toEqual({ kind: "up-to-date" });
  });

  it("passes the configured check timeout to check()", async () => {
    mockedCheck.mockResolvedValue(null);
    await checkForUpdate();
    expect(mockedCheck).toHaveBeenCalledWith({ timeout: CHECK_TIMEOUT_MS });
    expect(CHECK_TIMEOUT_MS).toBe(20_000);
    expect(DOWNLOAD_TIMEOUT_MS).toBe(120_000);
  });

  it("returns available with metadata + the Update handle", async () => {
    const u = fakeUpdate({ version: "0.3.0", body: "hi", date: "2026-06-01" });
    mockedCheck.mockResolvedValue(u as never);
    const r = await checkForUpdate();
    expect(r).toEqual({ kind: "available", version: "0.3.0", notes: "hi", date: "2026-06-01", update: u });
  });

  it("defaults notes/date to empty strings", async () => {
    const u = fakeUpdate({ body: undefined as unknown as string, date: undefined as unknown as string });
    mockedCheck.mockResolvedValue(u as never);
    const r = await checkForUpdate();
    expect(r.kind).toBe("available");
    if (r.kind === "available") {
      expect(r.notes).toBe("");
      expect(r.date).toBe("");
    }
  });

  it("returns error when check() throws", async () => {
    mockedCheck.mockRejectedValue(new Error("network down"));
    expect(await checkForUpdate()).toEqual({ kind: "error", message: "network down" });
  });
});

describe("applyUpdate", () => {
  it("downloads, installs, relaunches → updated", async () => {
    const u = fakeUpdate();
    mockedRelaunch.mockResolvedValue(undefined);
    const r = await applyUpdate(u as never);
    expect(u.downloadAndInstall).toHaveBeenCalledTimes(1);
    expect(mockedRelaunch).toHaveBeenCalledTimes(1);
    expect(r).toEqual({ kind: "updated", version: "0.2.1" });
  });

  it("passes download timeout via options", async () => {
    const u = fakeUpdate();
    await applyUpdate(u as never);
    const [, opts] = u.downloadAndInstall.mock.calls[0];
    expect(opts).toEqual({ timeout: DOWNLOAD_TIMEOUT_MS });
  });

  it("maps a relaunch rejection to needs-manual-restart", async () => {
    const u = fakeUpdate();
    mockedRelaunch.mockRejectedValue(new Error("sandbox"));
    const r = await applyUpdate(u as never);
    expect(r).toEqual({ kind: "needs-manual-restart", version: "0.2.1" });
  });

  it("returns error when downloadAndInstall throws", async () => {
    const u = fakeUpdate({ downloadAndInstall: vi.fn().mockRejectedValue(new Error("disk full")) });
    const r = await applyUpdate(u as never);
    expect(r).toEqual({ kind: "error", message: "disk full" });
    expect(mockedRelaunch).not.toHaveBeenCalled();
  });

  it("forwards progress: captures contentLength from Started, emits on Progress", async () => {
    const u = fakeUpdate({
      downloadAndInstall: vi.fn().mockImplementation(async (onEvent: (e: unknown) => void) => {
        onEvent({ event: "Started", data: { contentLength: 1000 } });
        onEvent({ event: "Progress", data: { chunkLength: 250 } });
        onEvent({ event: "Progress", data: { chunkLength: 250 } });
      }),
    });
    const seen: { chunkLength: number; contentLength?: number }[] = [];
    await applyUpdate(u as never, (p) => seen.push(p));
    expect(seen).toEqual([
      { chunkLength: 250, contentLength: 1000 },
      { chunkLength: 250, contentLength: 1000 },
    ]);
  });
});
