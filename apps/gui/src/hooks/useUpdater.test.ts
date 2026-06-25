import { createElement, type ReactNode } from "react";
import { describe, it, expect, vi } from "vitest";
import { renderHook, waitFor } from "@testing-library/react";

vi.mock("../lib/updaterClient", () => ({ checkForUpdate: vi.fn().mockResolvedValue({ kind: "up-to-date" }), applyUpdate: vi.fn() }));
vi.mock("@tauri-apps/api/window", () => ({ getCurrentWindow: () => ({ onFocusChanged: vi.fn().mockResolvedValue(() => {}) }) }));
vi.mock("@tauri-apps/api/app", () => ({ getVersion: vi.fn().mockResolvedValue("0.0.2") }));

import { useUpdater } from "./useUpdater";
import { UpdaterProvider } from "../api/UpdaterProvider";

const wrapper = ({ children }: { children: ReactNode }) => createElement(UpdaterProvider, null, children);

describe("useUpdater", () => {
  it("returns the provider context value", async () => {
    const { result } = renderHook(() => useUpdater(), { wrapper });
    // The provider's mount effect runs a check (checking -> up-to-date), so we
    // wait for the settled state; checkNow/applyNow are stable context fns.
    await waitFor(() => expect(result.current.status.state).toBe("up-to-date"));
    await waitFor(() => expect(result.current.currentVersion).toBe("0.0.2"));
    expect(typeof result.current.checkNow).toBe("function");
    expect(typeof result.current.applyNow).toBe("function");
  });

  it("returns a safe idle default when used outside a provider", () => {
    const { result } = renderHook(() => useUpdater());
    expect(result.current.status.state).toBe("idle");
  });
});
