import { describe, expect, it, vi, beforeEach } from "vitest";

const { invokeMock, requestCloseMock } = vi.hoisted(() => ({
  invokeMock: vi.fn().mockResolvedValue(undefined),
  requestCloseMock: vi.fn(),
}));

vi.mock("./paletteRuntime", () => ({
  createPanelBridgeRuntime: () => ({
    invoke: invokeMock,
    subscribe: vi.fn().mockReturnValue(() => {}),
    requestClose: requestCloseMock,
  }),
}));

import { requestPanelClose, requestShowGui } from "./panelWindowOps";

describe("panelWindowOps", () => {
  beforeEach(() => {
    invokeMock.mockReset();
    invokeMock.mockResolvedValue(undefined);
    requestCloseMock.mockReset();
  });

  it("requestPanelClose calls runtime.requestClose", async () => {
    await requestPanelClose();
    expect(requestCloseMock).toHaveBeenCalled();
  });

  it("requestShowGui invokes desktop_host_show_gui", async () => {
    await requestShowGui();
    expect(invokeMock).toHaveBeenCalledWith("desktop_host_show_gui");
  });
});
