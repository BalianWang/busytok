import { invoke } from "@tauri-apps/api/core";
import { save } from "@tauri-apps/plugin-dialog";
import { domToBlob } from "modern-screenshot";
import { useState, type RefObject } from "react";
import { reportFrontendEventSafely } from "../../logging/safeReporter";

export interface ReceiptExportApi {
  busy: boolean;
  savePng: () => Promise<void>;
}

const log = (event_code: string, message: string, details: Record<string, unknown>, level: "INFO" | "ERROR" = "INFO") =>
  reportFrontendEventSafely({ level, event_code, message, details });

export function useReceiptExport(
  target: RefObject<HTMLElement | null>,
  date: string,
): ReceiptExportApi {
  const [busy, setBusy] = useState(false);

  async function captureBytes(): Promise<Uint8Array> {
    const node = target.current;
    if (!node) throw new Error("receipt export target not mounted");
    // Deterministic font + paint: load each face explicitly (document.fonts.ready
    // alone can resolve before a not-yet-referenced face is fetched), then
    // double-rAF so layout/paint commit before the clone is serialized.
    const fonts = document.fonts;
    if (fonts) {
      await Promise.all([
        fonts.load('400 1em "BusytokMono"'),
        fonts.load('600 1em "BusytokMono"'),
        fonts.load('700 1em "BusytokMono"'),
        fonts.load('400 1em "BusytokSans"'),
        fonts.load('700 1em "BusytokSans"'),
      ]).catch((error) => {
        // A failed font load produces a silently degraded export (fallback
        // face with wrong metrics). Surface it so the operator can correlate
        // "why does my exported PNG look wrong" reports with a real event.
        log(
          "gui.receipt.font_load_failed",
          "receipt font preload failed",
          { date, error: error instanceof Error ? error.message : String(error) },
          "ERROR",
        );
      });
      await fonts.ready.catch(() => {});
    }
    await new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r)));
    // backgroundColor: null → transparent PNG. The .receipt-paper element
    // carries its own gradient + shadow; capturing paper (not the stage
    // wrapper) yields an image filled edge-to-edge by the receipt body.
    const blob = await domToBlob(node, {
      scale: 3,
      backgroundColor: null,
      font: { preferredFormat: "woff2" },
    });
    return new Uint8Array(await blob.arrayBuffer());
  }

  async function run(action: string, fn: () => Promise<void>) {
    setBusy(true);
    try {
      await fn();
    } catch (error) {
      const error_message = error instanceof Error ? error.message : String(error);
      log(`gui.receipt.${action}_failed`, `receipt ${action} failed`, { date, error_message }, "ERROR");
    } finally {
      setBusy(false);
    }
  }

  return {
    busy,
    async savePng() {
      await run("exported", async () => {
        const path = await save({
          defaultPath: `busytok-receipt-${date}.png`,
          filters: [{ name: "PNG Image", extensions: ["png"] }],
        });
        if (!path) return; // user cancelled
        const bytes = await captureBytes();
        // Tauri v2 IPC serialises invoke args via JSON. A bare Uint8Array is
        // stringified as {"0":1,"1":2,...} and fails to deserialize as
        // Vec<u8> on the Rust side → save silently rejects. Convert to a
        // plain JS array so serde decodes the bytes correctly.
        await invoke("save_receipt_png", { path, bytes: Array.from(bytes) });
        log("gui.receipt.exported", "receipt saved to file", { date, path });
      });
    },
  };
}
