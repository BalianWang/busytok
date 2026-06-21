//! BusytokClient backed by the panel bridge transport.
//!
//! Uses `createBusytokClient` with an `invoke` adapter that routes
//! `invoke_busytok` calls through `createPanelBridgeRuntime().invoke()`.

import { createBusytokClient } from "./busytokClient";
import { createPanelBridgeRuntime, type PaletteRuntime } from "../lib/paletteRuntime";

let _runtime: PaletteRuntime | null = null;

function getRuntime(): PaletteRuntime {
  if (!_runtime) {
    _runtime = createPanelBridgeRuntime();
  }
  return _runtime;
}

export const panelBusytokClient = createBusytokClient({
  invoke: async (cmd: string, args?: Record<string, unknown>) => {
    const runtime = getRuntime();
    if (cmd === "invoke_busytok") {
      const { method, params } = (args ?? {}) as { method: string; params: unknown };
      return runtime.invoke(method, params);
    }
    return runtime.invoke(cmd, args);
  },
});
