//! React context for BusytokClient injection.
//!
//! In the GUI dashboard the default `busytokClient` singleton is used.
//! In a WKWebView panel the tree is wrapped in `<BusytokClientProvider
//! client={panelBusytokClient}>` so every data hook transparently
//! resolves to the bridge-backed client without any signature changes.

import { createContext, useContext, type ReactNode } from "react";
import type { BusytokClient } from "./busytokClient";
import { busytokClient as defaultClient } from "./busytokClient";

/** Return type of `createBusytokClient` — mirrors the full API surface. */
export type { BusytokClient } from "./busytokClient";

const BusytokClientContext = createContext<BusytokClient>(defaultClient);

/** Provide a custom `BusytokClient` to a subtree. */
export function BusytokClientProvider({
  client,
  children,
}: {
  client: BusytokClient;
  children: ReactNode;
}) {
  return (
    <BusytokClientContext.Provider value={client}>
      {children}
    </BusytokClientContext.Provider>
  );
}

/** Resolve the current `BusytokClient`. Falls back to the default singleton. */
export function useBusytokClient(): BusytokClient {
  return useContext(BusytokClientContext);
}
