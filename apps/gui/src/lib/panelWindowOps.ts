//! Bridge window operations for the WKWebView panel context.
//!
//! Uses `createPanelBridgeRuntime()` to request close / show-GUI via
//! the native panel bridge instead of Tauri `invoke`.

import { createPanelBridgeRuntime, type PaletteRuntime } from "./paletteRuntime";

let _runtime: PaletteRuntime | null = null;

export function getPanelRuntime(): PaletteRuntime {
  if (!_runtime) {
    _runtime = createPanelBridgeRuntime();
  }
  return _runtime;
}

export async function requestPanelClose(): Promise<void> {
  const runtime = getPanelRuntime();
  await runtime.requestClose();
}

export async function requestShowGui(): Promise<void> {
  const runtime = getPanelRuntime();
  await runtime.invoke("desktop_host_show_gui");
}
