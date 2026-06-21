import { beforeEach, describe, expect, it, vi } from "vitest";
import { busytokClient } from "./busytokClient";
import { liveSamplesStore } from "./liveSamplesStore";
import { refreshLiveWindowSamples } from "./liveWindowRefresh";

vi.mock("./busytokClient", () => ({
  busytokClient: {
    liveWindow: vi.fn(),
  },
}));

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

function liveWindowEnvelope(tokensPerSec: number, generatedAtMs: number) {
  return {
    data: {
      exact_samples: [
        {
          bucket_start_ms: generatedAtMs * 1000,
          tokens_per_sec: tokensPerSec,
          cost_per_sec: null,
          events_per_sec: 1,
        },
      ],
      transient_samples: [],
      current_tokens_per_sec: tokensPerSec,
      current_events_per_sec: 1,
      start_ms: 0,
      end_ms: 0,
    },
    generated_at_ms: generatedAtMs,
    generation_id: null,
    readiness: "ready_exact" as const,
    is_exact: true,
    is_stale: false,
    watermark_ms: null,
    progress: null,
    degraded_reason: null,
  };
}

describe("refreshLiveWindowSamples", () => {
  beforeEach(() => {
    liveSamplesStore.setAll([]);
    vi.clearAllMocks();
  });

  it("runs a queued refresh even when the in-flight request fails", async () => {
    const first = deferred<Awaited<ReturnType<typeof busytokClient.liveWindow>>>();
    const second = deferred<Awaited<ReturnType<typeof busytokClient.liveWindow>>>();
    const liveWindow = vi.mocked(busytokClient.liveWindow);
    liveWindow
      .mockReturnValueOnce(first.promise)
      .mockReturnValueOnce(second.promise);

    const refresh = refreshLiveWindowSamples();
    const queuedRefresh = refreshLiveWindowSamples();

    first.reject(new Error("temporary live.window failure"));

    await vi.waitFor(() => {
      expect(liveWindow).toHaveBeenCalledTimes(2);
    });

    second.resolve(liveWindowEnvelope(42, 200));

    await expect(refresh).resolves.toBeUndefined();
    await expect(queuedRefresh).resolves.toBeUndefined();
    expect(liveSamplesStore.getSamples()[0]?.tokens_per_sec).toBe(42);
  });

  it("rejects when the only refresh attempt fails", async () => {
    vi.mocked(busytokClient.liveWindow).mockRejectedValueOnce(new Error("offline"));

    await expect(refreshLiveWindowSamples()).rejects.toThrow("offline");
  });
});
