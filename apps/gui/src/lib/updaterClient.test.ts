import { describe, it, expect, vi, beforeEach } from "vitest";

vi.mock("@tauri-apps/plugin-updater", () => ({
  check: vi.fn(),
}));

vi.mock("@tauri-apps/plugin-process", () => ({
  relaunch: vi.fn(),
}));

import { check } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";
import {
  checkAndApplyUpdate,
  initUpdaterAutoCheck,
  _testOnlyResetLatch,
} from "./updaterClient";

const mockedCheck = vi.mocked(check);
const mockedRelaunch = vi.mocked(relaunch);

beforeEach(() => {
  vi.clearAllMocks();
  _testOnlyResetLatch();
});

describe("checkAndApplyUpdate", () => {
  it("returns up-to-date when no update available", async () => {
    mockedCheck.mockResolvedValue(null);
    const result = await checkAndApplyUpdate();
    expect(result).toEqual({ kind: "up-to-date" });
    expect(mockedRelaunch).not.toHaveBeenCalled();
  });

  it("downloads, installs, and relaunches when update available", async () => {
    const fakeUpdate = {
      version: "0.2.1",
      downloadAndInstall: vi.fn().mockResolvedValue(undefined),
    };
    mockedCheck.mockResolvedValue(fakeUpdate as never);
    mockedRelaunch.mockResolvedValue(undefined);

    const result = await checkAndApplyUpdate();
    expect(fakeUpdate.downloadAndInstall).toHaveBeenCalledTimes(1);
    expect(mockedRelaunch).toHaveBeenCalledTimes(1);
    expect(result).toEqual({ kind: "updated", version: "0.2.1" });
  });

  it("returns error when check throws", async () => {
    mockedCheck.mockRejectedValue(new Error("network failed"));
    const result = await checkAndApplyUpdate();
    expect(result.kind).toBe("error");
    expect(mockedRelaunch).not.toHaveBeenCalled();
  });

  it("returns error when downloadAndInstall throws", async () => {
    const fakeUpdate = {
      version: "0.2.1",
      downloadAndInstall: vi.fn().mockRejectedValue(new Error("disk full")),
    };
    mockedCheck.mockResolvedValue(fakeUpdate as never);
    const result = await checkAndApplyUpdate();
    expect(result.kind).toBe("error");
    expect(mockedRelaunch).not.toHaveBeenCalled();
  });
});

describe("initUpdaterAutoCheck (module-level latch)", () => {
  it("fires check exactly once across multiple calls", async () => {
    mockedCheck.mockResolvedValue(null);
    initUpdaterAutoCheck();
    initUpdaterAutoCheck();
    initUpdaterAutoCheck();
    await vi.waitFor(() => {
      expect(mockedCheck).toHaveBeenCalledTimes(1);
    });
  });

  it("can refire after _testOnlyResetLatch", async () => {
    mockedCheck.mockResolvedValue(null);
    initUpdaterAutoCheck();
    await vi.waitFor(() => expect(mockedCheck).toHaveBeenCalledTimes(1));
    _testOnlyResetLatch();
    initUpdaterAutoCheck();
    await vi.waitFor(() => expect(mockedCheck).toHaveBeenCalledTimes(2));
  });
});
