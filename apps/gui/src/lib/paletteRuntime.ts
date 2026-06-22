//! Transport abstraction for the WKWebView panel context.
//!
//! The `PaletteRuntime` interface exposes invoke / subscribe / requestClose
//! operations backed by a `busytokPanelBridge` object that the native side
//! injects onto `window` before the panel HTML loads.

/** Bridge-level event dispatched from native to JS. */
interface PanelBridgeEvent {
  requestId?: string;
  type: string;
  payload: unknown;
}

/** Shape of the bridge object injected by the native panel host. */
interface PanelBridgeAPI {
  invoke(method: string, payload?: unknown): Promise<{
    ok: boolean;
    data?: unknown;
    error?: string;
  }>;
  subscribe(event: string, handler: (payload: unknown) => void): () => void;
}

declare global {
  interface Window {
    busytokPanelBridge?: PanelBridgeAPI;
    __busytokPanelBridgeDispatch?: (event: PanelBridgeEvent) => void;
  }
}

/** Transport abstraction consumed by panel-specific clients. */
export interface PaletteRuntime {
  invoke(method: string, payload?: unknown): Promise<unknown>;
  subscribe(event: string, handler: (payload: unknown) => void): () => void;
  requestClose(): Promise<void>;
}

/** Build a `PaletteRuntime` backed by `window.busytokPanelBridge`. */
export function createPanelBridgeRuntime(): PaletteRuntime {
  const bridge = window.busytokPanelBridge;

  return {
    async invoke(method: string, payload?: unknown): Promise<unknown> {
      if (!bridge) throw new Error("Panel bridge not available");
      const response = await bridge.invoke(method, payload);
      if (!response.ok) throw new Error(response.error ?? "Unknown error");
      return response.data;
    },

    subscribe(event: string, handler: (payload: unknown) => void): () => void {
      if (!bridge) return () => {};
      return bridge.subscribe(event, handler);
    },

    async requestClose(): Promise<void> {
      await bridge?.invoke("palette:close");
    },
  };
}
