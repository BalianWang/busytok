import { describe, expect, it, vi, beforeEach, afterEach } from "vitest";
import { renderHook, act } from "@testing-library/react";
import { liveSamplesStore } from "./liveSamplesStore";
import { useLiveSamples } from "./useLiveSamples";
import { busytokClient } from "./busytokClient";

vi.mock("./busytokClient", () => ({
  busytokClient: {
    liveWindow: vi.fn().mockResolvedValue({
      data: {
        exact_samples: [
          { bucket_start_ms: 1000, tokens_per_sec: 10, cost_per_sec: null, events_per_sec: 1 },
          { bucket_start_ms: 3000, tokens_per_sec: 20, cost_per_sec: null, events_per_sec: 2 },
        ],
        transient_samples: [],
        current_tokens_per_sec: 0,
        current_events_per_sec: 0,
        start_ms: 0,
        end_ms: 0,
      },
      generated_at_ms: 0,
      generation_id: null,
      readiness: "ready_exact",
      is_exact: false,
      is_stale: false,
      watermark_ms: null,
      progress: null,
      degraded_reason: null,
    }),
  },
}));

describe("useLiveSamples", () => {
  beforeEach(() => {
    liveSamplesStore.setAll([]);
    vi.clearAllMocks();
    vi.useRealTimers();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("starts with isLoading true before backfill resolves", () => {
    const { result } = renderHook(() => useLiveSamples());
    expect(result.current.isLoading).toBe(true);
  });

  it("backfills on mount and sets isLoading false", async () => {
    const { result } = renderHook(() => useLiveSamples());
    await vi.waitFor(() => {
      expect(result.current.isLoading).toBe(false);
    });
    expect(result.current.samples).toHaveLength(2);
    expect(result.current.samples[0].tokens_per_sec).toBe(10);
    expect(result.current.smoothedSamples).toHaveLength(2);
    expect(result.current.smoothedSamples.map((s) => s.tokens_per_min)).toEqual([600, 900]);
    expect(result.current.smoothedSamples.map((s) => s.raw_tokens_per_min)).toEqual([600, 1200]);
  });

});

describe("liveSamplesStore (ring buffer merge semantics)", () => {
  beforeEach(() => {
    liveSamplesStore.setAll([]);
  });

  it("setAll populates the store", () => {
    liveSamplesStore.setAll([
      { bucket_start_ms: 1000, tokens_per_sec: 10, cost_per_sec: null, events_per_sec: 1 },
    ]);
    expect(liveSamplesStore.getSamples()).toHaveLength(1);
  });

  it("replaceExact clears stale exact samples while preserving transient live edge", () => {
    liveSamplesStore.upsertExact({
      bucket_start_ms: 1000,
      tokens_per_sec: 10,
      cost_per_sec: null,
      events_per_sec: 1,
    });
    liveSamplesStore.upsertTransient({
      bucket_start_ms: 2000,
      tokens_per_sec: 20,
      cost_per_sec: null,
      events_per_sec: 2,
    });

    liveSamplesStore.replaceExact([
      { bucket_start_ms: 3000, tokens_per_sec: 30, cost_per_sec: null, events_per_sec: 3 },
    ]);

    const samples = liveSamplesStore.getSamplesWithFlags();
    expect(samples.map((s) => s.bucket_start_ms)).toEqual([2000, 3000]);
    expect(samples.find((s) => s.bucket_start_ms === 1000)).toBeUndefined();
    expect(samples.find((s) => s.bucket_start_ms === 2000)?.is_exact).toBe(false);
    expect(samples.find((s) => s.bucket_start_ms === 3000)?.is_exact).toBe(true);
  });

  it("replaceExact lets exact samples replace transient entries for the same bucket", () => {
    liveSamplesStore.upsertTransient({
      bucket_start_ms: 1000,
      tokens_per_sec: 20,
      cost_per_sec: null,
      events_per_sec: 2,
    });

    liveSamplesStore.replaceExact([
      { bucket_start_ms: 1000, tokens_per_sec: 30, cost_per_sec: null, events_per_sec: 3 },
    ]);

    const samples = liveSamplesStore.getSamplesWithFlags();
    expect(samples).toHaveLength(1);
    expect(samples[0].bucket_start_ms).toBe(1000);
    expect(samples[0].tokens_per_sec).toBe(30);
    expect(samples[0].is_exact).toBe(true);
  });

  it("replaceExact ignores stale window responses older than the latest applied response", () => {
    liveSamplesStore.replaceExact(
      [{ bucket_start_ms: 2000, tokens_per_sec: 40, cost_per_sec: null, events_per_sec: 4 }],
      { generatedAtMs: 200 },
    );

    liveSamplesStore.replaceExact(
      [{ bucket_start_ms: 1000, tokens_per_sec: 10, cost_per_sec: null, events_per_sec: 1 }],
      { generatedAtMs: 100 },
    );

    const samples = liveSamplesStore.getSamplesWithFlags();
    expect(samples).toHaveLength(1);
    expect(samples[0].bucket_start_ms).toBe(2000);
    expect(samples[0].tokens_per_sec).toBe(40);
  });

  it("upsert replaces on same bucket_start_ms", () => {
    liveSamplesStore.setAll([
      { bucket_start_ms: 1000, tokens_per_sec: 10, cost_per_sec: null, events_per_sec: 1 },
      { bucket_start_ms: 3000, tokens_per_sec: 20, cost_per_sec: null, events_per_sec: 2 },
    ]);

    liveSamplesStore.upsert({
      bucket_start_ms: 3000,
      tokens_per_sec: 99,
      cost_per_sec: null,
      events_per_sec: 5,
    });

    const samples = liveSamplesStore.getSamples();
    expect(samples).toHaveLength(2);
    expect(samples.find((s) => s.bucket_start_ms === 3000)?.tokens_per_sec).toBe(99);
  });

  it("upsert appends new bucket_start_ms", () => {
    liveSamplesStore.setAll([
      { bucket_start_ms: 1000, tokens_per_sec: 10, cost_per_sec: null, events_per_sec: 1 },
    ]);

    liveSamplesStore.upsert({
      bucket_start_ms: 5000,
      tokens_per_sec: 30,
      cost_per_sec: null,
      events_per_sec: 3,
    });

    expect(liveSamplesStore.getSamples()).toHaveLength(2);
  });

  it("evicts oldest when exceeding MAX_POINTS", () => {
    const points = Array.from({ length: 450 }, (_, i) => ({
      bucket_start_ms: (i + 1) * 2000,
      tokens_per_sec: 1,
      cost_per_sec: null as null,
      events_per_sec: 0,
    }));
    liveSamplesStore.setAll(points);

    liveSamplesStore.upsert({
      bucket_start_ms: 451 * 2000,
      tokens_per_sec: 99,
      cost_per_sec: null,
      events_per_sec: 1,
    });

    const samples = liveSamplesStore.getSamples();
    expect(samples).toHaveLength(450);
    expect(samples[0].bucket_start_ms).not.toBe(2000); // oldest evicted
    expect(samples[449].bucket_start_ms).toBe(451 * 2000);
  });

  it("subscribe returns unsubscribe that stops notifications", () => {
    const listener = vi.fn();
    const unsub = liveSamplesStore.subscribe(listener);
    unsub();
    liveSamplesStore.upsert({
      bucket_start_ms: 1000,
      tokens_per_sec: 10,
      cost_per_sec: null,
      events_per_sec: 1,
    });
    expect(listener).not.toHaveBeenCalled();
  });

  // ── Exact vs transient sample tests ──────────────────────────────

  it("upsertExact replaces or inserts with exact=true", () => {
    liveSamplesStore.upsertExact({
      bucket_start_ms: 1000,
      tokens_per_sec: 10,
      cost_per_sec: null,
      events_per_sec: 1,
    });
    const samples = liveSamplesStore.getSamples();
    expect(samples).toHaveLength(1);
    expect(samples[0].tokens_per_sec).toBe(10);
  });

  it("upsertTransient does not overwrite an existing exact sample", () => {
    liveSamplesStore.upsertExact({
      bucket_start_ms: 1000,
      tokens_per_sec: 10,
      cost_per_sec: null,
      events_per_sec: 1,
    });

    liveSamplesStore.upsertTransient({
      bucket_start_ms: 1000,
      tokens_per_sec: 50,
      cost_per_sec: null,
      events_per_sec: 5,
    });

    const samples = liveSamplesStore.getSamples();
    expect(samples).toHaveLength(1);
    // Transient should not overwrite exact
    expect(samples[0].tokens_per_sec).toBe(10);
  });

  it("transient sample can be upserted when no exact exists", () => {
    liveSamplesStore.upsertTransient({
      bucket_start_ms: 1000,
      tokens_per_sec: 30,
      cost_per_sec: null,
      events_per_sec: 3,
    });

    const samples = liveSamplesStore.getSamples();
    expect(samples).toHaveLength(1);
    expect(samples[0].tokens_per_sec).toBe(30);
  });

  it("exact sample overwrites a transient at the same bucket", () => {
    liveSamplesStore.upsertTransient({
      bucket_start_ms: 1000,
      tokens_per_sec: 30,
      cost_per_sec: null,
      events_per_sec: 3,
    });

    liveSamplesStore.upsertExact({
      bucket_start_ms: 1000,
      tokens_per_sec: 99,
      cost_per_sec: null,
      events_per_sec: 10,
    });

    const samples = liveSamplesStore.getSamples();
    expect(samples).toHaveLength(1);
    // Exact should overwrite transient
    expect(samples[0].tokens_per_sec).toBe(99);
  });

  it("clearTransient removes only transient entries, keeps exact", () => {
    liveSamplesStore.upsertExact({
      bucket_start_ms: 1000,
      tokens_per_sec: 10,
      cost_per_sec: null,
      events_per_sec: 1,
    });
    liveSamplesStore.upsertTransient({
      bucket_start_ms: 2000,
      tokens_per_sec: 20,
      cost_per_sec: null,
      events_per_sec: 2,
    });
    liveSamplesStore.upsertTransient({
      bucket_start_ms: 3000,
      tokens_per_sec: 30,
      cost_per_sec: null,
      events_per_sec: 3,
    });

    expect(liveSamplesStore.getSamples()).toHaveLength(3);

    liveSamplesStore.clearTransient();

    const samples = liveSamplesStore.getSamples();
    expect(samples).toHaveLength(1);
    expect(samples[0].bucket_start_ms).toBe(1000);
    expect(samples[0].tokens_per_sec).toBe(10);
  });
});
