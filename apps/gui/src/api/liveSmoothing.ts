import type { LiveSampleDto } from "@busytok/protocol-types";

export type LiveSampleInput = LiveSampleDto & { is_exact?: boolean };

export interface LiveSmoothedSample {
  bucket_start_ms: number;
  tokens_per_min: number;
  cost_per_min: number | null;
  events_per_min: number;
  raw_tokens_per_min: number;
  raw_peak_tokens_per_min: number;
  raw_event_count: number;
  is_exact: boolean;
}

export interface LiveDisplaySample extends LiveSmoothedSample {
  display_tokens_per_min: number;
  display_cost_per_min: number | null;
  display_events_per_min: number;
}

export interface SmoothLiveSamplesOptions {
  smoothingBucketCount?: number;
  bucketMs?: number;
}

export interface DisplaySmoothingOptions {
  bucketMs?: number;
  riseTauMs?: number;
  fallTauMs?: number;
  historyKernelRadiusBuckets?: number;
  historySigmaBuckets?: number;
  liveEdgeBlendBuckets?: number;
}

const DEFAULT_SMOOTHING_BUCKET_COUNT = 15;
const DEFAULT_BUCKET_MS = 2000;
const DEFAULT_DISPLAY_RISE_TAU_MS = 20_000;
const DEFAULT_DISPLAY_FALL_TAU_MS = 4_000;
const DEFAULT_HISTORY_KERNEL_RADIUS_BUCKETS = 12;
const DEFAULT_HISTORY_SIGMA_BUCKETS = 6;
const DEFAULT_LIVE_EDGE_BLEND_BUCKETS = 12;

export function smoothLiveSamples(
  samples: readonly LiveSampleInput[],
  options: SmoothLiveSamplesOptions = {},
): LiveSmoothedSample[] {
  if (samples.length === 0) return [];

  const smoothingBucketCount = Math.max(
    1,
    Math.floor(options.smoothingBucketCount ?? DEFAULT_SMOOTHING_BUCKET_COUNT),
  );
  const bucketMs = Math.max(1, Math.floor(options.bucketMs ?? DEFAULT_BUCKET_MS));
  const sorted = [...samples].sort((a, b) => a.bucket_start_ms - b.bucket_start_ms);
  const byBucket = new Map(sorted.map((sample) => [sample.bucket_start_ms, sample]));
  const firstBucket = sorted[0].bucket_start_ms;
  const lastBucket = sorted[sorted.length - 1].bucket_start_ms;
  const denseSamples: LiveSampleInput[] = [];

  for (let cursor = firstBucket; cursor <= lastBucket; cursor += bucketMs) {
    denseSamples.push(byBucket.get(cursor) ?? {
      bucket_start_ms: cursor,
      tokens_per_sec: 0,
      cost_per_sec: null,
      events_per_sec: 0,
      is_exact: true,
    });
  }

  return denseSamples.map((sample, index) => {
    const start = Math.max(0, index - smoothingBucketCount + 1);
    const window = denseSamples.slice(start, index + 1);
    const denominator = window.length;

    const tokensPerSecSum = window.reduce((sum, s) => sum + s.tokens_per_sec, 0);
    const eventsPerSecSum = window.reduce((sum, s) => sum + s.events_per_sec, 0);
    const costValues = window
      .map((s) => s.cost_per_sec)
      .filter((value): value is number => value != null);
    const costPerSecSum = costValues.reduce((sum, value) => sum + value, 0);
    const rawPeakTokensPerMin = Math.max(
      ...window.map((s) => s.tokens_per_sec * 60),
    );

    return {
      bucket_start_ms: sample.bucket_start_ms,
      tokens_per_min: (tokensPerSecSum / denominator) * 60,
      cost_per_min: costValues.length > 0 ? (costPerSecSum / denominator) * 60 : null,
      events_per_min: (eventsPerSecSum / denominator) * 60,
      raw_tokens_per_min: sample.tokens_per_sec * 60,
      raw_peak_tokens_per_min: rawPeakTokensPerMin,
      raw_event_count: sample.events_per_sec * (bucketMs / 1000),
      is_exact: window.every((s) => s.is_exact !== false),
    };
  });
}

function alphaForDelta(deltaMs: number, tauMs: number): number {
  return 1 - Math.exp(-deltaMs / tauMs);
}

function stepDisplayValue(
  current: number,
  target: number,
  deltaMs: number,
  riseTauMs: number,
  fallTauMs: number,
): number {
  const tauMs = target >= current ? riseTauMs : fallTauMs;
  return current + (target - current) * alphaForDelta(deltaMs, tauMs);
}

function gaussianWeight(distanceBuckets: number, sigmaBuckets: number): number {
  return Math.exp(-(distanceBuckets * distanceBuckets) / (2 * sigmaBuckets * sigmaBuckets));
}

function centeredSmoothNumber(
  samples: readonly LiveSmoothedSample[],
  index: number,
  radiusBuckets: number,
  sigmaBuckets: number,
  valueOf: (sample: LiveSmoothedSample) => number,
): number {
  let weightedSum = 0;
  let weightSum = 0;

  for (let offset = -radiusBuckets; offset <= radiusBuckets; offset += 1) {
    const sample = samples[index + offset];
    if (!sample) continue;
    const weight = gaussianWeight(offset, sigmaBuckets);
    weightedSum += valueOf(sample) * weight;
    weightSum += weight;
  }

  return weightSum > 0 ? weightedSum / weightSum : valueOf(samples[index]);
}

function centeredSmoothOptionalNumber(
  samples: readonly LiveSmoothedSample[],
  index: number,
  radiusBuckets: number,
  sigmaBuckets: number,
  valueOf: (sample: LiveSmoothedSample) => number | null,
): number | null {
  let weightedSum = 0;
  let weightSum = 0;

  for (let offset = -radiusBuckets; offset <= radiusBuckets; offset += 1) {
    const sample = samples[index + offset];
    if (!sample) continue;
    const value = valueOf(sample);
    if (value == null) continue;
    const weight = gaussianWeight(offset, sigmaBuckets);
    weightedSum += value * weight;
    weightSum += weight;
  }

  return weightSum > 0 ? weightedSum / weightSum : null;
}

function blendNumbers(historyValue: number, causalValue: number, liveWeight: number): number {
  return historyValue * (1 - liveWeight) + causalValue * liveWeight;
}

function blendOptionalNumbers(
  historyValue: number | null,
  causalValue: number | null,
  liveWeight: number,
): number | null {
  if (historyValue == null && causalValue == null) return null;
  if (historyValue == null) return causalValue;
  if (causalValue == null) return historyValue;
  return blendNumbers(historyValue, causalValue, liveWeight);
}

function liveEdgeWeight(index: number, totalCount: number, blendBuckets: number): number {
  if (blendBuckets <= 0) return 0;
  const distanceFromLiveEdge = totalCount - 1 - index;
  if (distanceFromLiveEdge >= blendBuckets) return 0;
  return 1 - distanceFromLiveEdge / blendBuckets;
}

export function buildDisplayLiveCurveSamples(
  samples: readonly LiveSmoothedSample[],
  options: DisplaySmoothingOptions = {},
): LiveDisplaySample[] {
  if (samples.length === 0) return [];

  const bucketMs = Math.max(1, Math.floor(options.bucketMs ?? DEFAULT_BUCKET_MS));
  const riseTauMs = Math.max(
    bucketMs,
    Math.floor(options.riseTauMs ?? DEFAULT_DISPLAY_RISE_TAU_MS),
  );
  const fallTauMs = Math.max(
    bucketMs,
    Math.floor(options.fallTauMs ?? DEFAULT_DISPLAY_FALL_TAU_MS),
  );
  const historyKernelRadiusBuckets = Math.max(
    0,
    Math.floor(options.historyKernelRadiusBuckets ?? DEFAULT_HISTORY_KERNEL_RADIUS_BUCKETS),
  );
  const historySigmaBuckets = Math.max(
    1,
    Math.floor(options.historySigmaBuckets ?? DEFAULT_HISTORY_SIGMA_BUCKETS),
  );
  const liveEdgeBlendBuckets = Math.max(
    0,
    Math.floor(options.liveEdgeBlendBuckets ?? DEFAULT_LIVE_EDGE_BLEND_BUCKETS),
  );
  const sorted = [...samples].sort((a, b) => a.bucket_start_ms - b.bucket_start_ms);

  let displayTokens = sorted[0].tokens_per_min;
  let displayCost = sorted[0].cost_per_min;
  let displayEvents = sorted[0].events_per_min;

  const causalSamples = sorted.map((sample, index) => {
    if (index > 0) {
      const previous = sorted[index - 1];
      const deltaMs = Math.max(bucketMs, sample.bucket_start_ms - previous.bucket_start_ms);
      displayTokens = stepDisplayValue(
        displayTokens,
        sample.tokens_per_min,
        deltaMs,
        riseTauMs,
        fallTauMs,
      );
      displayEvents = stepDisplayValue(
        displayEvents,
        sample.events_per_min,
        deltaMs,
        riseTauMs,
        fallTauMs,
      );
      displayCost =
        sample.cost_per_min == null
          ? null
          : stepDisplayValue(
              displayCost ?? 0,
              sample.cost_per_min,
              deltaMs,
              riseTauMs,
              fallTauMs,
            );
    }

    return {
      ...sample,
      display_tokens_per_min: displayTokens,
      display_cost_per_min: displayCost,
      display_events_per_min: displayEvents,
    };
  });

  if (historyKernelRadiusBuckets === 0) {
    return causalSamples;
  }

  return causalSamples.map((sample, index) => {
    const liveWeight = liveEdgeWeight(index, causalSamples.length, liveEdgeBlendBuckets);
    const historyTokens = centeredSmoothNumber(
      sorted,
      index,
      historyKernelRadiusBuckets,
      historySigmaBuckets,
      (s) => s.tokens_per_min,
    );
    const historyCost = centeredSmoothOptionalNumber(
      sorted,
      index,
      historyKernelRadiusBuckets,
      historySigmaBuckets,
      (s) => s.cost_per_min,
    );
    const historyEvents = centeredSmoothNumber(
      sorted,
      index,
      historyKernelRadiusBuckets,
      historySigmaBuckets,
      (s) => s.events_per_min,
    );

    return {
      ...sample,
      display_tokens_per_min: blendNumbers(
        historyTokens,
        sample.display_tokens_per_min,
        liveWeight,
      ),
      display_cost_per_min: blendOptionalNumbers(
        historyCost,
        sample.display_cost_per_min,
        liveWeight,
      ),
      display_events_per_min: blendNumbers(
        historyEvents,
        sample.display_events_per_min,
        liveWeight,
      ),
    };
  });
}
