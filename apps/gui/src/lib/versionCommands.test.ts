import { describe, it, expect, vi, beforeEach } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

import { invoke } from "@tauri-apps/api/core";
import { installVersion, listAvailableVersions } from "./versionCommands";

const mockedInvoke = vi.mocked(invoke);

beforeEach(() => vi.clearAllMocks());

describe("installVersion", () => {
  it("maps an Installed outcome", async () => {
    mockedInvoke.mockResolvedValue({ kind: "installed", version: "0.1.0-rc.4" });
    expect(await installVersion("https://x/latest.json")).toEqual({ kind: "installed", version: "0.1.0-rc.4" });
    expect(mockedInvoke).toHaveBeenCalledWith("install_version", { manifestUrl: "https://x/latest.json" });
  });

  it("maps a Failed outcome", async () => {
    mockedInvoke.mockResolvedValue({ kind: "failed", message: "sig bad" });
    expect(await installVersion("u")).toEqual({ kind: "failed", message: "sig bad" });
  });

  it("maps a thrown invoke to failed", async () => {
    mockedInvoke.mockRejectedValue(new Error("boom"));
    expect(await installVersion("u")).toEqual({ kind: "failed", message: "boom" });
  });

  it("maps a non-Error thrown value to failed", async () => {
    mockedInvoke.mockRejectedValue("string thrown");
    expect(await installVersion("u")).toEqual({ kind: "failed", message: "string thrown" });
  });

});

describe("listAvailableVersions", () => {
  it("invokes the list_available_versions command and returns the entries", async () => {
    const entries = [
      { version: "v0.0.2", date: "d2", notes: "n2", manifest_url: "u2" },
      { version: "v0.0.1", date: "d1", notes: "n1", manifest_url: "u1" },
    ];
    mockedInvoke.mockResolvedValue(entries);
    await expect(listAvailableVersions()).resolves.toEqual(entries);
    expect(mockedInvoke).toHaveBeenCalledWith("list_available_versions");
  });

  it("rejects when invoke throws, so useVersionHistory surfaces isError", async () => {
    mockedInvoke.mockRejectedValue(new Error("versions.json request failed"));
    await expect(listAvailableVersions()).rejects.toThrow("versions.json request failed");
  });
});
