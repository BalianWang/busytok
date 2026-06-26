import { cleanup, renderHook } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { useReceiptExport } from "./useReceiptExport";
import { toReceiptViewModel } from "./viewModel";
import { NORMAL_DAY } from "./fixtures";

const domToBlob = vi.fn();
const writeImage = vi.fn();
const save = vi.fn();
const invoke = vi.fn();
const reportEvent = vi.fn();

vi.mock("modern-screenshot", () => ({ domToBlob: (...a: unknown[]) => domToBlob(...a) }));
vi.mock("@tauri-apps/plugin-clipboard-manager", () => ({ writeImage: (...a: unknown[]) => writeImage(...a) }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ save: (...a: unknown[]) => save(...a) }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: (...a: unknown[]) => invoke(...a) }));
vi.mock("../../logging/safeReporter", () => ({
  reportFrontendEventSafely: (...a: unknown[]) => reportEvent(...a),
}));

afterEach(() => {
  cleanup();
  domToBlob.mockReset();
  writeImage.mockReset();
  save.mockReset();
  invoke.mockReset();
  reportEvent.mockReset();
});

function blob(bytes: number[]) {
  return new Blob([new Uint8Array(bytes)], { type: "image/png" });
}

describe("useReceiptExport", () => {
  it("copyImage: fonts.ready → domToBlob → writeImage, logs gui.receipt.copied", async () => {
    domToBlob.mockResolvedValue(blob([1, 2, 3]));
    const el = document.createElement("div");
    const { result } = renderHook(() =>
      useReceiptExport({ current: el }, toReceiptViewModel(NORMAL_DAY), "2026-06-26"),
    );
    await result.current.copyImage();
    expect(domToBlob).toHaveBeenCalledWith(
      el,
      expect.objectContaining({
        scale: 3,
        backgroundColor: "#E9E4DA",
        font: { preferredFormat: "woff2" },
      }),
    );
    expect(writeImage).toHaveBeenCalledWith(expect.any(Uint8Array));
    expect(reportEvent).toHaveBeenCalledWith(expect.objectContaining({ event_code: "gui.receipt.copied" }));
  });

  it("savePng: save() → invoke save_receipt_png, logs gui.receipt.exported", async () => {
    domToBlob.mockResolvedValue(blob([9, 9]));
    save.mockResolvedValue("/tmp/x.png");
    const el = document.createElement("div");
    const { result } = renderHook(() =>
      useReceiptExport({ current: el }, toReceiptViewModel(NORMAL_DAY), "2026-06-26"),
    );
    await result.current.savePng();
    expect(save).toHaveBeenCalled();
    expect(invoke).toHaveBeenCalledWith("save_receipt_png", expect.objectContaining({ path: "/tmp/x.png" }));
    expect(reportEvent).toHaveBeenCalledWith(expect.objectContaining({ event_code: "gui.receipt.exported" }));
  });

  it("savePng does nothing when the user cancels the dialog", async () => {
    save.mockResolvedValue(null);
    const el = document.createElement("div");
    const { result } = renderHook(() =>
      useReceiptExport({ current: el }, toReceiptViewModel(NORMAL_DAY), "2026-06-26"),
    );
    await result.current.savePng();
    expect(invoke).not.toHaveBeenCalled();
  });

  it("copySummary writes a text summary", async () => {
    const writeText = vi.fn();
    vi.stubGlobal("navigator", { ...navigator, clipboard: { writeText } });
    const el = document.createElement("div");
    const { result } = renderHook(() =>
      useReceiptExport({ current: el }, toReceiptViewModel(NORMAL_DAY), "2026-06-26"),
    );
    await result.current.copySummary();
    expect(writeText).toHaveBeenCalledWith(expect.stringContaining("Busytok"));
    vi.unstubAllGlobals();
  });

  it("copyImage logs gui.receipt.copied_failed when domToBlob rejects (no throw)", async () => {
    domToBlob.mockRejectedValueOnce(new Error("capture failed"));
    const el = document.createElement("div");
    const { result } = renderHook(() =>
      useReceiptExport({ current: el }, toReceiptViewModel(NORMAL_DAY), "2026-06-26"),
    );
    await expect(result.current.copyImage()).resolves.toBeUndefined();
    expect(reportEvent).toHaveBeenCalledWith(
      expect.objectContaining({
        level: "ERROR",
        event_code: "gui.receipt.copied_failed",
      }),
    );
  });

  it("copyImage logs gui.receipt.copied_failed when writeImage rejects (no throw)", async () => {
    domToBlob.mockResolvedValueOnce(blob([1, 2, 3]));
    writeImage.mockRejectedValueOnce(new Error("write failed"));
    const el = document.createElement("div");
    const { result } = renderHook(() =>
      useReceiptExport({ current: el }, toReceiptViewModel(NORMAL_DAY), "2026-06-26"),
    );
    await expect(result.current.copyImage()).resolves.toBeUndefined();
    expect(reportEvent).toHaveBeenCalledWith(
      expect.objectContaining({
        level: "ERROR",
        event_code: "gui.receipt.copied_failed",
      }),
    );
  });

  it("savePng logs gui.receipt.exported_failed when invoke rejects (no throw)", async () => {
    domToBlob.mockResolvedValueOnce(blob([9, 9]));
    save.mockResolvedValueOnce("/tmp/x.png");
    invoke.mockRejectedValueOnce(new Error("invoke failed"));
    const el = document.createElement("div");
    const { result } = renderHook(() =>
      useReceiptExport({ current: el }, toReceiptViewModel(NORMAL_DAY), "2026-06-26"),
    );
    await expect(result.current.savePng()).resolves.toBeUndefined();
    expect(reportEvent).toHaveBeenCalledWith(
      expect.objectContaining({
        level: "ERROR",
        event_code: "gui.receipt.exported_failed",
      }),
    );
  });
});
