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

export interface TurnAutoParams {
  logical_subagent_id: string;
  logical_subagent_name?: string;
  cwd: string;
  profile: string;
  model?: string;
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
