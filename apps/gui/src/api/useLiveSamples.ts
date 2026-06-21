import { useEffect, useMemo, useState, useSyncExternalStore } from "react";
import type { LiveSampleDto } from "@busytok/protocol-types";
import { liveSamplesStore } from "./liveSamplesStore";
import { refreshLiveWindowSamples } from "./liveWindowRefresh";
import { smoothLiveSamples, type LiveSmoothedSample } from "./liveSmoothing";

export type LiveSampleWithFlags = LiveSampleDto & { is_exact: boolean };

export function useLiveSamples(): {
  samples: readonly LiveSampleWithFlags[];
  smoothedSamples: readonly LiveSmoothedSample[];
  isLoading: boolean;
  hasTransient: boolean;
} {
  const [isLoading, setIsLoading] = useState(true);

  useEffect(() => {
    let cancelled = false;

    async function loadInitialWindow() {
      try {
        await refreshLiveWindowSamples();
      } catch {
        // Push updates handle ongoing refresh; this is just the initial load.
      } finally {
        if (!cancelled) setIsLoading(false);
      }
    }

    void loadInitialWindow();

    return () => {
      cancelled = true;
    };
  }, []);

  const samplesWithFlags = useSyncExternalStore(
    liveSamplesStore.subscribe,
    liveSamplesStore.getSamplesWithFlags,
  );

  const smoothedSamples = useMemo(
    () => smoothLiveSamples(samplesWithFlags),
    [samplesWithFlags],
  );

  const hasTransient = samplesWithFlags.some((s) => !s.is_exact);

  return { samples: samplesWithFlags, smoothedSamples, isLoading, hasTransient };
}
