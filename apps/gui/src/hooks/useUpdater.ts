import { useContext } from "react";
import { UpdaterContext, type UpdaterContextValue, type UpdaterStatus } from "../api/UpdaterProvider";

export type { UpdaterContextValue, UpdaterStatus };

/**
 * Single consumer hook for update state. Reads the context's safe idle default
 * when no provider is present (mirrors EventSubscriptionProvider) — so the badge
 * renders null in isolation tests rather than throwing.
 */
export function useUpdater(): UpdaterContextValue {
  return useContext(UpdaterContext);
}
