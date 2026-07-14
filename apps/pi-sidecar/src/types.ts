export interface JsonRpcRequest {
  jsonrpc: '2.0';
  method: string;
  params?: unknown;
  id?: number;
}

export interface JsonRpcResponse {
  jsonrpc: '2.0';
  result?: unknown;
  error?: JsonRpcError;
  id: number;
}

export interface JsonRpcError {
  code: number;
  message: string;
  data?: unknown;
}

export const PROTOCOL_VERSION = 1;

export interface InitializeResult {
  protocol_version: number;
  sidecar_version: string;
  pi_version?: string;
}

export interface HealthResult {
  status: 'healthy' | 'unhealthy';
  sessions: number;
  rss_mb: number;
}

export interface KeyFile {
  path: string;
  reason: string;
  last_seen_at_ms: number;
  score: number;
}

export interface OpenQuestion {
  question: string;
  status: 'open' | 'resolved';
  created_at_ms: number;
  last_seen_at_ms: number;
}

export interface MemoryField {
  hot_summary?: string;
  long_summary?: string;
  key_files: KeyFile[];
  decisions: string[];
  open_questions: OpenQuestion[];
}

export interface CompactContext {
  compact_context: string;
  budget_tokens: number;
  source: string;
}

export type ProviderKind = 'openai_compatible' | 'anthropic_compatible';

export interface TurnAutoParams {
  /** Stable task identity used to scope cancellation to this turn. */
  task_id?: string;
  logical_subagent_id: string;
  logical_subagent_name?: string;
  cwd: string;
  profile: string;
  /** Model ID — REQUIRED. The Rust side always sends the bound model
   *  (or model_override) since subagent binding makes routing explicit
   *  (spec §3.3 + §4.3). Tightened from optional in Phase 3. */
  model: string;
  /** Provider ID — now REQUIRED (was optional in Phase 3). The Rust side
   *  always sends it since subagent binding makes provider routing explicit. */
  provider_id: string;
  provider_kind: ProviderKind;
  provider_base_url: string;
  /** Transient — sidecar must NOT log this in plaintext. */
  provider_api_key: string;
  model_reasoning: boolean;
  model_context_window: number;
  model_max_tokens: number;
  model_display_name?: string;
  tools?: string[];
  prompt: string;
  prompt_artifact_ref?: string | null;
  timeout_ms?: number;
  memory?: MemoryField;
  context?: CompactContext;
  constraints?: { write_access: boolean; timeout_ms: number };
}

export interface MemoryUpdate {
  current_state_summary?: string;
  key_files?: KeyFile[];
  decisions?: string[];
  open_questions?: OpenQuestion[];
}

export interface TurnAutoResult {
  adapter_session_id: string;
  session_reused: boolean;
  status: 'completed' | 'failed' | 'timeout';
  result: {
    task_summary: string;
    memory_update?: MemoryUpdate;
    [key: string]: unknown;
  };
  usage: {
    model: string;
    provider: string;
    input_tokens: number;
    output_tokens: number;
    cache_read_tokens: number;
    cache_write_tokens: number;
    cost_usd: number;
  };
}

export interface PrepareHibernateParams {
  adapter_session_id?: string;
  all?: boolean;
}

export interface MemoryDelta {
  hot_summary?: string;
  key_files?: string[];
  decisions?: string[];
  open_questions?: string[];
}

export interface PrepareHibernateResult {
  // Single-session path (adapter_session_id provided)
  memory_delta?: MemoryDelta | null;
  stats: Record<string, unknown>;
  // All-sessions path (all:true) — per-session breakdown so the Rust
  // shutdown/idle-exit path can persist each session's memory delta
  // individually (spec §5.4). Plan 3 returns the shape; Plan 4 wires the
  // real ContextBuilder memory and the Rust-side consumer.
  sessions?: HibernateSessionEntry[];
}

export interface HibernateSessionEntry {
  adapter_session_id: string;
  logical_subagent_id: string;
  memory_delta: MemoryDelta | null;
  stats: Record<string, unknown>;
}

export interface CloseParams {
  adapter_session_id: string;
}

export interface CloseResult {
  ok: boolean;
}

export interface CancelParams {
  logical_subagent_id: string;
  /** When present, cancel only this task's active turn. */
  task_id?: string;
}

export interface CancelResult {
  cancelled: boolean;
}

export interface ActivateParams {
  adapter_session_id: string;
}

export interface ActivateResult {
  ok: boolean;
}

// New error codes start at -32010 to avoid collisions with existing
// protocol constants -32001..-32008 (SESSION_NOT_FOUND through
// PROTOCOL_MISMATCH) in Rust protocol.rs.
export const ERROR_CODE_AUTH_FAILURE = -32010;
export const ERROR_CODE_RATE_LIMIT = -32011;
export const ERROR_CODE_NETWORK = -32012;
export const ERROR_CODE_TURN_CANCELLED = -32013;
// -32003 remains TASK_TIMEOUT (reused for timeout classification)
// -32002 remains HOT_SESSION_LIMIT_REACHED
