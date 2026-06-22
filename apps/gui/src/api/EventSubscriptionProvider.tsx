import { createContext, useEffect, useRef, useState, type ReactNode } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  EventSubscriptionBatchDto,
} from "@busytok/protocol-types";
import { queryKeys } from "./queryKeys";
import { reportFrontendEvent } from "../logging/reporter";
import { liveSamplesStore } from "./liveSamplesStore";
import { refreshLiveWindowSamples } from "./liveWindowRefresh";

export type ConnectionStatus = "connected" | "disconnected" | "reconnecting";

export type ServiceConnectionStatus = "starting" | "repairing" | "ready" | "unavailable";
export type SubscriptionBridgeStatus = "connected" | "disconnected" | "reconnecting";

interface SubscriptionStatusEvent {
  status: ConnectionStatus;
  since_ms: number;
  latest_event_seq?: number | null;
  last_seen_seq?: number | null;
  gap_detected?: boolean;
  replayed_scopes?: Array<{ dataset: string; breakdown_kind?: string | null }>;
}

export interface ServiceStatusEvent {
  status: ServiceConnectionStatus;
  since_ms: number;
  reason?: string | null;
}

export interface EventSubscriptionContextValue {
  connectionStatus: ConnectionStatus;
  serviceStatus: ServiceConnectionStatus;
  bridgeStatus: SubscriptionBridgeStatus;
}

export const EventSubscriptionContext = createContext<EventSubscriptionContextValue>({
  connectionStatus: "disconnected",
  serviceStatus: "starting",
  bridgeStatus: "disconnected",
});

const DEBOUNCE_MS = 500;

/**
 * Scope-to-query-key mapping.
 *
 * Each invalidation scope emitted by the backend maps to one or more
 * TanStack Query key prefixes so that `queryClient.invalidateQueries`
 * can hit the right caches.
 */
function applyScopeInvalidation(
  queryClient: ReturnType<typeof useQueryClient>,
  dataset: string,
  breakdownKind: string | null,
): void {
  switch (dataset) {
    // ── Parent scopes (invalidate all children) ────────────────────
    case "overview":
      queryClient.invalidateQueries({ queryKey: ["overview"] });
      break;

    case "breakdown": {
      if (breakdownKind) {
        queryClient.invalidateQueries({
          queryKey: ["breakdown", "detail", { kind: breakdownKind }],
        });
      } else {
        // No breakdown_kind → parent scope → invalidate all breakdown children
        queryClient.invalidateQueries({ queryKey: ["breakdown"] });
      }
      break;
    }

    case "diagnostics":
      queryClient.invalidateQueries({ queryKey: queryKeys.settingsDiagnostics() });
      break;

    // ── Fine-grained scopes ────────────────────────────────────────
    case "overview_summary":
      queryClient.invalidateQueries({ queryKey: ["overview", "summary"] });
      break;

    case "overview_trend":
      queryClient.invalidateQueries({ queryKey: ["overview", "trend"] });
      break;

    case "overview_heatmap":
      queryClient.invalidateQueries({ queryKey: ["overview", "heatmap"] });
      break;

    case "overview_rankings":
      queryClient.invalidateQueries({ queryKey: ["overview", "rankings"] });
      break;

    case "activity":
      queryClient.invalidateQueries({ queryKey: ["activity"] });
      break;

    case "activity_recent":
      queryClient.invalidateQueries({ queryKey: ["activity", "recent"] });
      break;

    case "activity_list":
      queryClient.invalidateQueries({ queryKey: ["activity", "list"] });
      break;

    case "settings":
      queryClient.invalidateQueries({ queryKey: ["settings"] });
      break;

    case "settings_diagnostics":
      queryClient.invalidateQueries({ queryKey: queryKeys.settingsDiagnostics() });
      break;

    case "live_realtime":
      queryClient.invalidateQueries({ queryKey: queryKeys.liveWindow(null) });
      liveSamplesStore.clearTransient();
      break;

    default:
      break;
  }
}

function scopeAffectsShellStatus(dataset: string): boolean {
  switch (dataset) {
    case "overview":
    case "overview_summary":
    case "overview_trend":
    case "overview_heatmap":
    case "overview_rankings":
    case "activity":
    case "activity_recent":
    case "activity_list":
    case "breakdown":
    case "clients":
    case "live_realtime":
      return true;
    default:
      return false;
  }
}

/**
 * Invalidate all data queries and reload the live window.
 * Used when a sequence gap is detected with no replayed scopes.
 */
function invalidateAllDataQueries(queryClient: ReturnType<typeof useQueryClient>) {
  // shell.status powers the titlebar chips and is derived from the same runtime
  // data as overview/activity/live queries, so full data invalidation must
  // refresh it too.
  queryClient.invalidateQueries({ queryKey: queryKeys.shellStatus() });
  queryClient.invalidateQueries({ queryKey: ["overview"] });
  queryClient.invalidateQueries({ queryKey: ["activity"] });
  queryClient.invalidateQueries({ queryKey: ["breakdown"] });
  queryClient.invalidateQueries({ queryKey: ["settings"] });
  queryClient.invalidateQueries({ queryKey: ["live", "window"] });
}

export function EventSubscriptionProvider({ children }: { children: ReactNode }) {
  const queryClient = useQueryClient();
  const [serviceStatus, setServiceStatus] = useState<ServiceConnectionStatus>("starting");
  const [bridgeStatus, setBridgeStatus] = useState<SubscriptionBridgeStatus>("disconnected");
  const debounceRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const pendingScopesRef = useRef<Set<string>>(new Set());
  const lastEventSeqRef = useRef<number | null>(null);
  const disposedRef = useRef(false);
  const lastServiceStatusRef = useRef<ServiceConnectionStatus>("starting");
  const invalidatedOnTransitionRef = useRef(false);

  async function reloadLiveWindowSamples() {
    try {
      await refreshLiveWindowSamples();
    } catch {
      // Keep push samples and fallback polling alive; the next invalidation or
      // reconnect recovery can refresh the exact window again.
    }
  }

  /** Flush pending invalidation scopes after debounce. */
  function flushScopes() {
    const scopes = pendingScopesRef.current;
    pendingScopesRef.current = new Set();
    let shouldReloadLiveWindow = false;
    let shouldInvalidateShellStatus = false;
    for (const scopeKey of scopes) {
      const colonIdx = scopeKey.indexOf(":");
      const dataset = colonIdx >= 0 ? scopeKey.slice(0, colonIdx) : scopeKey;
      const breakdownKind = colonIdx >= 0 ? scopeKey.slice(colonIdx + 1) || null : null;
      if (dataset === "live_realtime") {
        shouldReloadLiveWindow = true;
      }
      applyScopeInvalidation(queryClient, dataset, breakdownKind);
      shouldInvalidateShellStatus =
        scopeAffectsShellStatus(dataset) || shouldInvalidateShellStatus;
    }
    if (shouldInvalidateShellStatus) {
      queryClient.invalidateQueries({ queryKey: queryKeys.shellStatus() });
    }
    if (shouldReloadLiveWindow) {
      void reloadLiveWindowSamples();
    }
  }

  useEffect(() => {
    let unlistenEvent: UnlistenFn | undefined;
    let unlistenStatus: UnlistenFn | undefined;
    let unlistenServiceStatus: UnlistenFn | undefined;
    disposedRef.current = false;

    async function setup() {
      unlistenEvent = await listen<EventSubscriptionBatchDto>("busytok:event", (event) => {
        const batch = event.payload;
        if (batch.events.length > 0) {
          setBridgeStatus("connected");
          // Latch service status to "ready" — receiving events implies the
          // service is running. This recovers from the race where the initial
          // busytok:service-status ("ready") was emitted before the listener
          // was registered (WebView mounts after backend connects).
          if (lastServiceStatusRef.current !== "ready") {
            const prev = lastServiceStatusRef.current;
            lastServiceStatusRef.current = "ready";
            setServiceStatus("ready");
            // Invalidate runtime queries — the overview query may still fail
            // on the first attempt (service is still scanning), but the
            // global retry config (retry:3 + exponential backoff) gives
            // up to ~7s of retries across the scan window.
            invalidateAllDataQueries(queryClient);
            reportFrontendEvent({
              level: "INFO",
              event_code: "gui.subscription.service_ready_latched_from_event_batch",
              message: "serviceStatus latched to ready from event batch (startup status event was missed)",
              details: { previous_status: prev, batch_size: batch.events.length },
            });
          }
        }
        for (const runtimeEvent of batch.events) {
          // ── Sequence gap detection ──────────────────────────────
          if (runtimeEvent.event_seq != null) {
            const prev = lastEventSeqRef.current;
            if (prev != null && runtimeEvent.event_seq > prev + 1) {
              // Gap detected in event stream
              if (!runtimeEvent.scopes || runtimeEvent.scopes.length === 0) {
                // No replayed scopes → invalidate everything
                invalidateAllDataQueries(queryClient);
                liveSamplesStore.clearTransient();
              }
            }
            lastEventSeqRef.current = runtimeEvent.event_seq;
          }

          // ── Scope-based invalidation ────────────────────────────
          if (runtimeEvent.event_type === "data:invalidated") {
            // Use envelope-level scopes if present, otherwise fall back
            // to the legacy payload.datasets field.
            const scopes = runtimeEvent.scopes && runtimeEvent.scopes.length > 0
              ? runtimeEvent.scopes
              : ((runtimeEvent.payload as any)?.datasets as Array<{ dataset: string; breakdown_kind?: string | null }> | undefined);

            if (scopes) {
              for (const scope of scopes) {
                const key = scope.breakdown_kind
                  ? `${scope.dataset}:${scope.breakdown_kind}`
                  : scope.dataset;
                pendingScopesRef.current.add(key);
              }
              if (debounceRef.current) clearTimeout(debounceRef.current);
              debounceRef.current = setTimeout(flushScopes, DEBOUNCE_MS);
            }
          }

          // ── Live window reloaded (reconnect backfill) ──────────
          if (runtimeEvent.event_type === "live:window_reloaded") {
            // The payload is the full live.window response: { data: { exact_samples, transient_samples }, ... }
            const payload = runtimeEvent.payload as any;
            const exact = payload?.data?.exact_samples as Array<{
              bucket_start_ms: number;
              tokens_per_sec: number;
              cost_per_sec?: number | null;
              events_per_sec: number;
            }> | undefined;
            if (exact) {
              liveSamplesStore.replaceExact(
                exact.map((s) => ({
                  bucket_start_ms: s.bucket_start_ms,
                  tokens_per_sec: s.tokens_per_sec,
                  cost_per_sec: s.cost_per_sec ?? null,
                  events_per_sec: s.events_per_sec,
                })),
                { generatedAtMs: payload?.generated_at_ms },
              );
            }
            const transient = payload?.data?.transient_samples as Array<{
              bucket_start_ms: number;
              tokens_per_sec: number;
              cost_per_sec?: number | null;
              events_per_sec: number;
            }> | undefined;
            if (transient) {
              for (const s of transient) {
                liveSamplesStore.upsertTransient({
                  bucket_start_ms: s.bucket_start_ms,
                  tokens_per_sec: s.tokens_per_sec,
                  cost_per_sec: s.cost_per_sec ?? null,
                  events_per_sec: s.events_per_sec,
                });
              }
            }
          }

          // ── Live sample handling (exact vs transient) ───────────
          if (runtimeEvent.event_type === "live:sample") {
            const payload = runtimeEvent.payload as any;
            const sample = {
              bucket_start_ms: payload.bucket_start_ms,
              tokens_per_sec: payload.tokens_per_sec,
              cost_per_sec: payload.cost_per_sec ?? null,
              events_per_sec: payload.events_per_sec,
            } as const;

            // `live:sample` events are ephemeral transport messages, so the
            // envelope-level `is_exact` is false even for exact aggregate
            // samples. The sample-level `transient` flag is the source of
            // truth for chart classification.
            const transient = payload.transient === true;
            const isExact = !transient;

            if (isExact) {
              liveSamplesStore.upsertExact(sample);
            } else {
              liveSamplesStore.upsertTransient(sample);
            }
          }
        }
      });

      unlistenStatus = await listen<SubscriptionStatusEvent>(
        "busytok:subscription-status",
        (event) => {
          const payload = event.payload;
          setBridgeStatus(payload.status as SubscriptionBridgeStatus);

          // Handle gap recovery on reconnection
          if (
            payload.status === "connected" &&
            payload.gap_detected &&
            payload.last_seen_seq != null &&
            payload.latest_event_seq != null
          ) {
            const replayedScopes = payload.replayed_scopes ?? [];
            if (replayedScopes.length === 0) {
              // Gap with no replayed scopes → invalidate all + clear transient
              invalidateAllDataQueries(queryClient);
              liveSamplesStore.clearTransient();
            } else {
              // Replayed scopes cover the gap — apply them
              for (const scope of replayedScopes) {
                const key = scope.breakdown_kind
                  ? `${scope.dataset}:${scope.breakdown_kind}`
                  : scope.dataset;
                pendingScopesRef.current.add(key);
              }
              if (debounceRef.current) clearTimeout(debounceRef.current);
              debounceRef.current = setTimeout(flushScopes, DEBOUNCE_MS);
            }
            // Update our last known sequence
            lastEventSeqRef.current = payload.latest_event_seq;
          }
        },
      );

      unlistenServiceStatus = await listen<ServiceStatusEvent>(
        "busytok:service-status",
        (event) => {
          const payload = event.payload;
          const newStatus = payload.status;
          const prevStatus = lastServiceStatusRef.current;
          lastServiceStatusRef.current = newStatus;
          setServiceStatus(newStatus);

          // Invalidate stale query groups once when service becomes non-ready
          if (newStatus !== "ready" && prevStatus === "ready" && !invalidatedOnTransitionRef.current) {
            invalidatedOnTransitionRef.current = true;
            invalidateAllDataQueries(queryClient);
            liveSamplesStore.clearTransient();
          }
          if (newStatus === "ready") {
            invalidatedOnTransitionRef.current = false;
            // Overview data queries may be stale (prefetched before the
            // service was ready); force a refetch on the ready transition.
            invalidateAllDataQueries(queryClient);
          }
        },
      );
    }

    setup();
    return () => {
      disposedRef.current = true;
      if (debounceRef.current) {
        clearTimeout(debounceRef.current);
      }
      unlistenEvent?.();
      unlistenStatus?.();
      unlistenServiceStatus?.();
    };
  }, [queryClient]);

  // Derive backward-compatible connectionStatus from both channels.
  // When service is not ready, report "disconnected" regardless of bridge state.
  // When bridge is reconnecting, report "reconnecting".
  // Otherwise, report the bridge status.
  const connectionStatus: ConnectionStatus =
    serviceStatus !== "ready"
      ? "disconnected"
      : bridgeStatus === "reconnecting"
        ? "reconnecting"
        : bridgeStatus;

  const value: EventSubscriptionContextValue = { connectionStatus, serviceStatus, bridgeStatus };

  return (
    <EventSubscriptionContext.Provider value={value}>
      {children}
    </EventSubscriptionContext.Provider>
  );
}
