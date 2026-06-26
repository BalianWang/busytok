import { invoke } from "@tauri-apps/api/core";
import { save } from "@tauri-apps/plugin-dialog";
import { writeImage } from "@tauri-apps/plugin-clipboard-manager";
import { domToBlob } from "modern-screenshot";
import { useState, type RefObject } from "react";
import { reportFrontendEventSafely } from "../../logging/safeReporter";
import type { ReceiptViewModel } from "./viewModel";

export interface ReceiptExportApi {
  busy: boolean;
  copyImage: () => Promise<void>;
  savePng: () => Promise<void>;
  copySummary: () => Promise<void>;
}

const log = (event_code: string, message: string, details: Record<string, unknown>, level: "INFO" | "ERROR" = "INFO") =>
  reportFrontendEventSafely({ level, event_code, message, details });

export function useReceiptExport(
  target: RefObject<HTMLElement | null>,
  vm: ReceiptViewModel,
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
        fonts.load('700 1em "BusytokMono"'),
        fonts.load('400 1em "BusytokSans"'),
        fonts.load('700 1em "BusytokSans"'),
      ]).catch(() => {});
      await fonts.ready.catch(() => {});
    }
    await new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r)));
    // Solid backgroundColor (the stage color) — null/transparent can render
    // black in some WebKit foreignObject paths.
    const blob = await domToBlob(node, {
      scale: 3,
      backgroundColor: "#E9E4DA",
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
    async copyImage() {
      await run("copied", async () => {
        const bytes = await captureBytes();
        await writeImage(bytes);
        log("gui.receipt.copied", "receipt copied to clipboard", { date });
      });
    },
    async savePng() {
      await run("exported", async () => {
        const path = await save({
          defaultPath: `busytok-receipt-${date}.png`,
          filters: [{ name: "PNG Image", extensions: ["png"] }],
        });
        if (!path) return; // user cancelled
        const bytes = await captureBytes();
        await invoke("save_receipt_png", { path, bytes });
        log("gui.receipt.exported", "receipt saved to file", { date, path });
      });
    },
    async copySummary() {
      await run("summary_copied", async () => {
        const text = [
          "Busytok — daily token receipt",
          vm.dateLabel,
          `Total tokens: ${vm.hero.totalTokens}`,
          vm.secondary.cost,
          `Top: ${vm.items.slice(0, 3).map((i) => i.name).join(", ")}`,
        ].join("\n");
        await navigator.clipboard.writeText(text);
        log("gui.receipt.summary_copied", "receipt summary copied", { date });
      });
    },
  };
}
