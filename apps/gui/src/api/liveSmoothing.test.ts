import { describe, expect, it } from "vitest";
import { buildDisplayLiveCurveSamples, smoothLiveSamples } from "./liveSmoothing";

describe("smoothLiveSamples", () => {
  it("uses partial trailing windows at the left edge", () => {
    const samples = [
      { bucket_start_ms: 0, tokens_per_sec: 10, cost_per_sec: null, events_per_sec: 1, is_exact: true },
      { bucket_start_ms: 2000, tokens_per_sec: 20, cost_per_sec: null, events_per_sec: 2, is_exact: true },
      { bucket_start_ms: 4000, tokens_per_sec: 30, cost_per_sec: null, events_per_sec: 3, is_exact: true },
    ];

    const result = smoothLiveSamples(samples, { smoothingBucketCount: 15 });

    expect(result.map((s) => s.tokens_per_min)).toEqual([600, 900, 1200]);
    expect(result.map((s) => s.events_per_min)).toEqual([60, 90, 120]);
    expect(result.map((s) => s.raw_event_count)).toEqual([2, 4, 6]);
    expect(result.map((s) => s.raw_tokens_per_min)).toEqual([600, 1200, 1800]);
  });

  it("spreads a single spike across the configured trailing window", () => {
    const samples = [
      { bucket_start_ms: 0, tokens_per_sec: 0, cost_per_sec: null, events_per_sec: 0, is_exact: true },
      { bucket_start_ms: 2000, tokens_per_sec: 90, cost_per_sec: null, events_per_sec: 1, is_exact: true },
      { bucket_start_ms: 4000, tokens_per_sec: 0, cost_per_sec: null, events_per_sec: 0, is_exact: true },
      { bucket_start_ms: 6000, tokens_per_sec: 0, cost_per_sec: null, events_per_sec: 0, is_exact: true },
    ];

    const result = smoothLiveSamples(samples, { smoothingBucketCount: 3 });

    expect(result.map((s) => s.tokens_per_min)).toEqual([0, 2700, 1800, 1800]);
    expect(result.map((s) => s.raw_peak_tokens_per_min)).toEqual([0, 5400, 5400, 5400]);
  });

  it("keeps a constant rate unchanged after smoothing", () => {
    const samples = Array.from({ length: 20 }, (_, i) => ({
      bucket_start_ms: i * 2000,
      tokens_per_sec: 12,
      cost_per_sec: 0.001,
      events_per_sec: 0.5,
      is_exact: true,
    }));

    const result = smoothLiveSamples(samples);

    expect(result.every((s) => s.tokens_per_min === 720)).toBe(true);
    for (const sample of result) {
      expect(sample.cost_per_min).toBeCloseTo(0.06);
    }
    expect(result.every((s) => s.events_per_min === 30)).toBe(true);
  });

  it("keeps cost null when the smoothing window has no priced samples", () => {
    const result = smoothLiveSamples([
      { bucket_start_ms: 0, tokens_per_sec: 10, cost_per_sec: null, events_per_sec: 1, is_exact: true },
      { bucket_start_ms: 2000, tokens_per_sec: 20, cost_per_sec: null, events_per_sec: 2, is_exact: true },
    ]);

    expect(result.map((s) => s.cost_per_min)).toEqual([null, null]);
  });

  it("densifies missing 2s buckets with zero values before smoothing", () => {
    const result = smoothLiveSamples(
      [
        { bucket_start_ms: 0, tokens_per_sec: 10, cost_per_sec: null, events_per_sec: 1, is_exact: true },
        { bucket_start_ms: 2000, tokens_per_sec: 20, cost_per_sec: null, events_per_sec: 2, is_exact: true },
        { bucket_start_ms: 10000, tokens_per_sec: 30, cost_per_sec: null, events_per_sec: 3, is_exact: true },
      ],
      { smoothingBucketCount: 3, bucketMs: 2000 },
    );

    expect(result.map((s) => s.bucket_start_ms)).toEqual([0, 2000, 4000, 6000, 8000, 10000]);
    expect(result.map((s) => s.tokens_per_min)).toEqual([600, 900, 600, 400, 0, 600]);
    expect(result.map((s) => s.raw_tokens_per_min)).toEqual([600, 1200, 0, 0, 0, 1800]);
  });

  it("sorts samples before densifying and smoothing", () => {
    const result = smoothLiveSamples(
      [
        { bucket_start_ms: 4000, tokens_per_sec: 30, cost_per_sec: null, events_per_sec: 3, is_exact: true },
        { bucket_start_ms: 0, tokens_per_sec: 10, cost_per_sec: null, events_per_sec: 1, is_exact: true },
        { bucket_start_ms: 2000, tokens_per_sec: 20, cost_per_sec: null, events_per_sec: 2, is_exact: true },
      ],
      { smoothingBucketCount: 2 },
    );

    expect(result.map((s) => s.bucket_start_ms)).toEqual([0, 2000, 4000]);
    expect(result.map((s) => s.tokens_per_min)).toEqual([600, 900, 1500]);
  });

  it("marks smoothed samples transient when any source bucket in the window is transient", () => {
    const result = smoothLiveSamples(
      [
        { bucket_start_ms: 0, tokens_per_sec: 10, cost_per_sec: null, events_per_sec: 1, is_exact: true },
        { bucket_start_ms: 2000, tokens_per_sec: 20, cost_per_sec: null, events_per_sec: 2, is_exact: false },
        { bucket_start_ms: 4000, tokens_per_sec: 30, cost_per_sec: null, events_per_sec: 3, is_exact: true },
      ],
      { smoothingBucketCount: 3 },
    );

    expect(result.map((s) => s.is_exact)).toEqual([true, false, false]);
  });

  it("filters display samples with causal asymmetric time constants", () => {
    const display = buildDisplayLiveCurveSamples([
      {
        bucket_start_ms: 0,
        tokens_per_min: 0,
        cost_per_min: null,
        events_per_min: 0,
        raw_tokens_per_min: 0,
        raw_peak_tokens_per_min: 0,
        raw_event_count: 0,
        is_exact: true,
      },
      {
        bucket_start_ms: 2000,
        tokens_per_min: 1000,
        cost_per_min: 10,
        events_per_min: 100,
        raw_tokens_per_min: 1000,
        raw_peak_tokens_per_min: 1000,
        raw_event_count: 1,
        is_exact: true,
      },
      {
        bucket_start_ms: 4000,
        tokens_per_min: 0,
        cost_per_min: null,
        events_per_min: 0,
        raw_tokens_per_min: 0,
        raw_peak_tokens_per_min: 0,
        raw_event_count: 0,
        is_exact: true,
      },
    ], {
      bucketMs: 2000,
      riseTauMs: 4000,
      fallTauMs: 20000,
      historyKernelRadiusBuckets: 0,
    });

    const riseAlpha = 1 - Math.exp(-2000 / 4000);
    const fallAlpha = 1 - Math.exp(-2000 / 20000);
    const expectedPeak = 1000 * riseAlpha;
    const expectedDecay = expectedPeak + (0 - expectedPeak) * fallAlpha;

    expect(display[0].display_tokens_per_min).toBe(0);
    expect(display[1].display_tokens_per_min).toBeCloseTo(expectedPeak);
    expect(display[2].display_tokens_per_min).toBeCloseTo(expectedDecay);
    expect(display[1].display_cost_per_min).toBeCloseTo(10 * riseAlpha);
    expect(display[2].display_cost_per_min).toBeNull();
  });

  it("keeps the newest display bucket causal even when a spike arrives", () => {
    const display = buildDisplayLiveCurveSamples([
      {
        bucket_start_ms: 0,
        tokens_per_min: 0,
        cost_per_min: null,
        events_per_min: 0,
        raw_tokens_per_min: 0,
        raw_peak_tokens_per_min: 0,
        raw_event_count: 0,
        is_exact: true,
      },
      {
        bucket_start_ms: 2000,
        tokens_per_min: 600000,
        cost_per_min: null,
        events_per_min: 0,
        raw_tokens_per_min: 3000000,
        raw_peak_tokens_per_min: 3000000,
        raw_event_count: 0,
        is_exact: true,
      },
    ]);

    expect(display[1].display_tokens_per_min).toBeGreaterThan(0);
    expect(display[1].display_tokens_per_min).toBeLessThan(600000);
  });

  it("softens historical sawtooth buckets for a calmer product curve", () => {
    const display = buildDisplayLiveCurveSamples([
      ...Array.from({ length: 48 }, (_, i) => ({
        bucket_start_ms: i * 2000,
        tokens_per_min: i < 10 ? 0 : i < 20 ? 600000 : i < 30 ? 300000 : i < 40 ? 900000 : 200000,
        cost_per_min: null,
        events_per_min: 0,
        raw_tokens_per_min: 0,
        raw_peak_tokens_per_min: 0,
        raw_event_count: 0,
        is_exact: true,
      })),
    ]);

    const historicalValues = display
      .slice(8, -14)
      .map((sample) => sample.display_tokens_per_min);
    const maxAdjacentJump = Math.max(
      ...historicalValues.slice(1).map((value, index) => Math.abs(value - historicalValues[index])),
    );

    expect(maxAdjacentJump).toBeLessThan(45_000);
  });

  it("shapes repeated sparse spikes into lower display peaks", () => {
    const display = buildDisplayLiveCurveSamples([
      {
        bucket_start_ms: 0,
        tokens_per_min: 0,
        cost_per_min: null,
        events_per_min: 0,
        raw_tokens_per_min: 0,
        raw_peak_tokens_per_min: 0,
        raw_event_count: 0,
        is_exact: true,
      },
      ...Array.from({ length: 20 }, (_, i) => ({
        bucket_start_ms: (i + 1) * 2000,
        tokens_per_min: i % 4 === 0 ? 600000 : 0,
        cost_per_min: null,
        events_per_min: 0,
        raw_tokens_per_min: i % 4 === 0 ? 3000000 : 0,
        raw_peak_tokens_per_min: i % 4 === 0 ? 3000000 : 0,
        raw_event_count: i % 4 === 0 ? 1 : 0,
        is_exact: true,
      })),
    ]);

    const maxInput = Math.max(...display.map((sample) => sample.tokens_per_min));
    const maxDisplay = Math.max(...display.map((sample) => sample.display_tokens_per_min));

    expect(maxDisplay).toBeGreaterThan(0);
    expect(maxDisplay).toBeLessThan(maxInput * 0.55);
  });

  it("keeps constant rolling-average values unchanged during display smoothing", () => {
    const base = Array.from({ length: 10 }, (_, i) => ({
      bucket_start_ms: i * 2000,
      tokens_per_min: 720,
      cost_per_min: 0.06,
      events_per_min: 30,
      raw_tokens_per_min: 720,
      raw_peak_tokens_per_min: 720,
      raw_event_count: 1,
      is_exact: true,
    }));

    const display = buildDisplayLiveCurveSamples(base);

    expect(display.every((sample) => Math.abs(sample.display_tokens_per_min - 720) < 0.0001)).toBe(true);
    expect(display.every((sample) => Math.abs((sample.display_cost_per_min ?? 0) - 0.06) < 0.0001)).toBe(true);
  });
});
