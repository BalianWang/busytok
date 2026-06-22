import type { LiveSampleDto } from "@busytok/protocol-types";

const MAX_POINTS = 450; // 15 min / 2s

type Listener = () => void;

interface SampleEntry {
  sample: LiveSampleDto;
  is_exact: boolean;
}

let samples: SampleEntry[] = [];
const listeners = new Set<Listener>();
let latestExactWindowGeneratedAtMs: number | null = null;

// Cached snapshot to satisfy useSyncExternalStore's referential stability
// contract. Invalidate on every mutation.
let cachedSnapshot: readonly LiveSampleDto[] | null = null;
let cachedSnapshotWithFlags: readonly (LiveSampleDto & { is_exact: boolean })[] | null = null;

function invalidateCache() {
  cachedSnapshot = null;
  cachedSnapshotWithFlags = null;
}

function notify() {
  invalidateCache();
  for (const l of listeners) l();
}

export const liveSamplesStore = {
  setAll(newSamples: LiveSampleDto[]) {
    latestExactWindowGeneratedAtMs = null;
    samples = newSamples.slice(-MAX_POINTS).map((s) => ({
      sample: s,
      is_exact: true,
    }));
    notify();
  },

  /** Replace exact samples from `live.window`, preserving live transient edge samples. */
  replaceExact(
    newExactSamples: LiveSampleDto[],
    options: { generatedAtMs?: number } = {},
  ) {
    if (
      options.generatedAtMs != null &&
      latestExactWindowGeneratedAtMs != null &&
      options.generatedAtMs < latestExactWindowGeneratedAtMs
    ) {
      return false;
    }

    if (options.generatedAtMs != null) {
      latestExactWindowGeneratedAtMs = options.generatedAtMs;
    }

    const exactBuckets = new Set(newExactSamples.map((s) => s.bucket_start_ms));
    const transient = samples.filter(
      (s) => !s.is_exact && !exactBuckets.has(s.sample.bucket_start_ms),
    );
    samples = [
      ...newExactSamples.map((sample) => ({ sample, is_exact: true })),
      ...transient,
    ]
      .sort((a, b) => a.sample.bucket_start_ms - b.sample.bucket_start_ms)
      .slice(-MAX_POINTS);
    notify();
    return true;
  },

  /** Upsert a sample with exact/transient semantics. Backward-compatible. */
  upsert(sample: LiveSampleDto) {
    const idx = samples.findIndex(
      (s) => s.sample.bucket_start_ms === sample.bucket_start_ms,
    );
    if (idx >= 0) {
      samples[idx] = { sample, is_exact: samples[idx].is_exact };
    } else {
      samples.push({ sample, is_exact: false });
      if (samples.length > MAX_POINTS) samples.shift();
    }
    notify();
  },

  /** Upsert an exact sample (from sampler). Overwrites transient at same bucket. */
  upsertExact(sample: LiveSampleDto) {
    const idx = samples.findIndex(
      (s) => s.sample.bucket_start_ms === sample.bucket_start_ms,
    );
    if (idx >= 0) {
      samples[idx] = { sample, is_exact: true };
    } else {
      samples.push({ sample, is_exact: true });
      if (samples.length > MAX_POINTS) samples.shift();
    }
    notify();
  },

  /** Upsert a transient sample (from tailer). Does NOT overwrite existing exact. */
  upsertTransient(sample: LiveSampleDto) {
    const idx = samples.findIndex(
      (s) => s.sample.bucket_start_ms === sample.bucket_start_ms,
    );
    if (idx >= 0) {
      // Do not overwrite an exact sample with transient data
      if (samples[idx].is_exact) return;
      samples[idx] = { sample, is_exact: false };
    } else {
      samples.push({ sample, is_exact: false });
      if (samples.length > MAX_POINTS) samples.shift();
    }
    notify();
  },

  /** Remove all transient entries, keeping only exact ones. */
  clearTransient() {
    samples = samples.filter((s) => s.is_exact);
    notify();
  },

  getSamples(): readonly LiveSampleDto[] {
    if (!cachedSnapshot) {
      cachedSnapshot = samples.map((s) => s.sample);
    }
    return cachedSnapshot;
  },

  /** Returns samples annotated with their exact/transient classification. */
  getSamplesWithFlags(): readonly (LiveSampleDto & { is_exact: boolean })[] {
    if (!cachedSnapshotWithFlags) {
      cachedSnapshotWithFlags = samples.map((s) => ({ ...s.sample, is_exact: s.is_exact }));
    }
    return cachedSnapshotWithFlags;
  },

  subscribe(listener: Listener): () => void {
    listeners.add(listener);
    return () => { listeners.delete(listener); };
  },
};
