import type { ReceiptDailyDto } from "@busytok/protocol-types";

export const NORMAL_DAY: ReceiptDailyDto = {
  date: "2026-06-26",
  date_label: "FRI · JUN 26, 2026",
  timezone: "Asia/Shanghai",
  metrics: {
    total_tokens: 3_412_888, input_tokens: 2_100_000, output_tokens: 912_888,
    cache_read_tokens: 1_800_000, cache_creation_tokens: 60_000,
    cache_hit_rate: 0.4615, cost_usd: 47.21, cost_status: "exact",
    event_count: 312, session_count: 14,
    peak_hour: { label: "14:00", tokens: 612_000 },
  },
  top_models: [
    { name: "claude-sonnet-4-5", tokens: 1_820_442, cost_usd: 24.1, cost_status: "exact" },
    { name: "claude-haiku-4-5", tokens: 810_200, cost_usd: 1.55, cost_status: "exact" },
    { name: "gpt-5.1", tokens: 530_000, cost_usd: 18.4, cost_status: "exact" },
    { name: "deepseek-v3.2", tokens: 252_246, cost_usd: 3.16, cost_status: "exact" },
  ],
  brand: { name: "BUSYTOK", tagline: "AI CODING · TOKEN RECEIPT", github: "github.com/BalianWang/busytok", generated_at_ms: 1_781_600_000_000 },
};

export const MANY_MODELS: ReceiptDailyDto = {
  ...NORMAL_DAY,
  top_models: [
    ...NORMAL_DAY.top_models,
    { name: "gemini-2.5-pro", tokens: 200_000, cost_usd: 2.0, cost_status: "exact" },
    { name: "model-six", tokens: 150_000, cost_usd: 1.0, cost_status: "exact" },
    { name: "model-seven", tokens: 100_000, cost_usd: 0.5, cost_status: "exact" },
    { name: "model-eight", tokens: 50_000, cost_usd: 0.1, cost_status: "exact" },
  ],
};

export const PARTIAL_COST: ReceiptDailyDto = {
  ...NORMAL_DAY,
  metrics: { ...NORMAL_DAY.metrics, cost_status: "partial" },
  top_models: [
    { name: "claude-sonnet-4-5", tokens: 1_820_442, cost_usd: 24.1, cost_status: "exact" },
    { name: "no-price-model", tokens: 810_200, cost_usd: null, cost_status: "unavailable" },
  ],
};

export const ZERO_COST: ReceiptDailyDto = {
  ...NORMAL_DAY,
  metrics: { ...NORMAL_DAY.metrics, cost_usd: null, cost_status: "unavailable", cache_hit_rate: null },
  top_models: [{ name: "free-local-model", tokens: 12_000, cost_usd: null, cost_status: "unavailable" }],
};

export const LONG_NAMES: ReceiptDailyDto = {
  ...NORMAL_DAY,
  top_models: [
    { name: "claude-sonnet-4-5-thinking-very-long-identifier-2026", tokens: 1_000_000, cost_usd: 10, cost_status: "exact" },
    { name: "another-extremely-long-and-descriptive-model-name", tokens: 500_000, cost_usd: 5, cost_status: "exact" },
  ],
};

export const NO_DATA: ReceiptDailyDto = {
  date: "2026-06-26",
  date_label: "FRI · JUN 26, 2026",
  timezone: "UTC",
  metrics: {
    total_tokens: 0, input_tokens: 0, output_tokens: 0, cache_read_tokens: 0,
    cache_creation_tokens: 0, cache_hit_rate: null, cost_usd: null,
    cost_status: "unavailable", event_count: 0, session_count: 0, peak_hour: null,
  },
  top_models: [],
  brand: { name: "BUSYTOK", tagline: "AI CODING · TOKEN RECEIPT", github: "github.com/BalianWang/busytok", generated_at_ms: 0 },
};

// >5 models where the overflow (models 6-7) are ALL unavailable → the OTHERS
// row's aggregate cost_status must be "unavailable" (render "—"), not "partial".
export const OTHERS_ALL_UNAVAILABLE: ReceiptDailyDto = {
  ...NORMAL_DAY,
  top_models: [
    { name: "m1", tokens: 500, cost_usd: 5, cost_status: "exact" },
    { name: "m2", tokens: 400, cost_usd: 4, cost_status: "exact" },
    { name: "m3", tokens: 300, cost_usd: 3, cost_status: "exact" },
    { name: "m4", tokens: 250, cost_usd: 2.5, cost_status: "exact" },
    { name: "m5", tokens: 200, cost_usd: 2, cost_status: "exact" },
    { name: "free-a", tokens: 150, cost_usd: null, cost_status: "unavailable" },
    { name: "free-b", tokens: 120, cost_usd: null, cost_status: "unavailable" },
  ],
};
