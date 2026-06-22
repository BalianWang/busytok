import { vi } from "vitest";

if (!globalThis.crypto?.randomUUID) {
  vi.stubGlobal("crypto", {
    ...globalThis.crypto,
    randomUUID: () => "00000000-0000-4000-8000-000000000000",
  });
}

// Polyfill Pointer Events APIs used by Radix UI primitives (Select, etc.)
// that jsdom does not implement.
if (typeof HTMLElement !== "undefined") {
  HTMLElement.prototype.hasPointerCapture = HTMLElement.prototype.hasPointerCapture ?? (() => false);
  HTMLElement.prototype.setPointerCapture = HTMLElement.prototype.setPointerCapture ?? (() => {});
  HTMLElement.prototype.releasePointerCapture = HTMLElement.prototype.releasePointerCapture ?? (() => {});
  HTMLElement.prototype.scrollIntoView = HTMLElement.prototype.scrollIntoView ?? (() => {});
}

// Polyfill ResizeObserver used by Radix UI Tooltip/Popper.
if (typeof globalThis.ResizeObserver === "undefined") {
  globalThis.ResizeObserver = class ResizeObserver {
    observe() {}
    unobserve() {}
    disconnect() {}
  } as unknown as typeof globalThis.ResizeObserver;
}
