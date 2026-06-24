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

import { useEffect, useRef, useState, type ReactNode } from "react";
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
import { reportFrontendEventSafely } from "../logging/safeReporter";

export type {
  ConnectionStatus,
  ServiceConnectionStatus,
};

/** Panel version of the event subscription provider. */
export function PanelEventSubscriptionProvider({ children }: { children: ReactNode }) {
  const queryClient = useQueryClient();
  const [serviceStatus, setServiceStatus] = useState<ServiceConnectionStatus>("starting");
  // Mirrors serviceStatus for a stable read inside the latch effect below.
  const lastServiceStatusRef = useRef<ServiceConnectionStatus>("starting");

  useEffect(() => {
    const runtime = createPanelBridgeRuntime();

    const unsubServiceStatus = runtime.subscribe(
      "service:status",
      (payload: unknown) => {
        const event = payload as ServiceStatusEvent;
        lastServiceStatusRef.current = event.status;
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

  // Latch recovery (mirrors EventSubscriptionProvider's latch). The panel
  // bridge subscribe has no retained-event replay, so if the native
  // service:status=ready push lands before React subscribes it is dropped and
  // serviceStatus stays "starting" — falsely blocking prompt actions. A
  // successful prompts query (loaded by pull, so it succeeds regardless of the
  // missed push) proves the service is alive; latch "ready" so the action gate
  // (PromptPaletteOverlayController) does not falsely block.
  //
  // Tightened to a NEW success only: a high-water mark on dataUpdatedAt means
  // only a prompts success with dataUpdatedAt newer than any already observed
  // (and newer than this subscription) latches. This blocks stale cached
  // success — pre-subscription OR from earlier in this provider's lifetime —
  // from re-latching "ready" after the service genuinely became "unavailable".
  // The panel QueryClient is a module-level singleton whose cache persists
  // across overlay remounts, so observer reattach / invalidate / focus would
  // otherwise re-emit an old success (status "success", fetchStatus "idle",
  // UNCHANGED dataUpdatedAt) and falsely unblock paste actions.
  // NOTE: this is the panel-side latch, consistent with the main window — not
  // the bridge retain/replay endgame (tracked separately).
  useEffect(() => {
    let newestSeen = Date.now(); // ignore success data older than subscription
    const unsubscribe = queryClient.getQueryCache().subscribe((event) => {
      const state = event.query.state;
      const key = event.query.queryKey;
      if (
        !Array.isArray(key) ||
        key[0] !== "prompts" ||
        state.status !== "success" ||
        state.fetchStatus !== "idle" ||
        state.dataUpdatedAt <= newestSeen
      ) {
        return;
      }
      newestSeen = state.dataUpdatedAt;
      if (lastServiceStatusRef.current !== "ready") {
        lastServiceStatusRef.current = "ready";
        setServiceStatus("ready");
        reportFrontendEventSafely({
          level: "INFO",
          event_code: "gui.subscription.panel_service_ready_latched_from_prompts_query",
          message:
            "Panel serviceStatus latched to ready from a fresh prompts query success (startup service:status push was missed)",
        });
      }
    });
    return unsubscribe;
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
