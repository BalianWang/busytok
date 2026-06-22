//! Bridge-backed event subscription provider for the WKWebView panel context.
//!
//! Provides the **same** `EventSubscriptionContext` that
//! `useEventSubscription()` reads, so components work unchanged whether
//! they run inside the Tauri GUI dashboard or inside a WKWebView panel.
//!
//! Subscribes via `createPanelBridgeRuntime().subscribe()` instead of
//! Tauri `listen()`.
//!
//! Service status states: `starting` / `ready` / `unavailable`.
//! Reconnection is handled by the host's subscribe.rs and abstracted away
//! from the panel — the panel only observes the three states above.

import { useEffect, useState, type ReactNode } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { createPanelBridgeRuntime } from "../lib/paletteRuntime";
import {
  EventSubscriptionContext,
  type EventSubscriptionContextValue,
  type ConnectionStatus,
  type ServiceConnectionStatus,
  type ServiceStatusEvent,
} from "./EventSubscriptionProvider";
import { queryKeys } from "./queryKeys";

export type {
  ConnectionStatus,
  ServiceConnectionStatus,
};

/** Panel version of the event subscription provider. */
export function PanelEventSubscriptionProvider({ children }: { children: ReactNode }) {
  const queryClient = useQueryClient();
  const [serviceStatus, setServiceStatus] = useState<ServiceConnectionStatus>("starting");

  useEffect(() => {
    const runtime = createPanelBridgeRuntime();

    const unsubServiceStatus = runtime.subscribe(
      "service:status",
      (payload: unknown) => {
        const event = payload as ServiceStatusEvent;
        setServiceStatus(event.status);
      },
    );

    const unsubPromptsInvalidate = runtime.subscribe(
      "prompts:invalidate",
      () => {
        queryClient.invalidateQueries({ queryKey: queryKeys.promptsRoot() });
      },
    );

    return () => {
      unsubServiceStatus();
      unsubPromptsInvalidate();
    };
  }, [queryClient]);

  const connectionStatus: ConnectionStatus =
    serviceStatus === "ready" ? "connected" : "disconnected";

  const value: EventSubscriptionContextValue = {
    connectionStatus,
    serviceStatus,
    bridgeStatus: connectionStatus,
  };

  return (
    <EventSubscriptionContext.Provider value={value}>
      {children}
    </EventSubscriptionContext.Provider>
  );
}
