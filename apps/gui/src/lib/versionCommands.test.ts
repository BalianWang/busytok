import { describe, it, expect, vi, beforeEach } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

import { invoke } from "@tauri-apps/api/core";
import { installVersion, VERSIONS_MANIFEST_URL } from "./versionCommands";

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

  it("exposes the manifest URL constant", () => {
    expect(VERSIONS_MANIFEST_URL).toMatch(/releases\/latest\/download\/versions\.json$/);
  });
});
