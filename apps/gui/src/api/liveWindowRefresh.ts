import { busytokClient } from "./busytokClient";
import { liveSamplesStore } from "./liveSamplesStore";

let refreshPromise: Promise<void> | null = null;
let refreshPending = false;

async function applyLiveWindow() {
  const envelope = await busytokClient.liveWindow({ window_seconds: 900 });
  liveSamplesStore.replaceExact(envelope.data.exact_samples, {
    generatedAtMs: envelope.generated_at_ms,
  });
  for (const sample of envelope.data.transient_samples) {
    liveSamplesStore.upsertTransient(sample);
  }
}

export function refreshLiveWindowSamples(): Promise<void> {
  if (refreshPromise) {
    refreshPending = true;
    return refreshPromise;
  }

  refreshPromise = (async () => {
    let lastError: unknown = null;
    do {
      refreshPending = false;
      try {
        await applyLiveWindow();
        lastError = null;
      } catch (error) {
        lastError = error;
      }
    } while (refreshPending);

    if (lastError) {
      throw lastError;
    }
  })().finally(() => {
    refreshPromise = null;
  });

  return refreshPromise;
}
