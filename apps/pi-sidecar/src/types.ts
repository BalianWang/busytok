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
}

export interface TurnAutoResult {
  adapter_session_id: string;
  session_reused: boolean;
  status: 'completed' | 'failed' | 'timeout';
  result: {
    task_summary: string;
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
