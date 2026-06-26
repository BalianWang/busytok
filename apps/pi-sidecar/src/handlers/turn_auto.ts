import { type TurnAutoParams, type TurnAutoResult } from '../types.js';
import type { RequestHandler } from '../rpc.js';
import { SidecarError } from '../errors.js';
import type { SessionPool } from '../session_pool.js';

let sessionCounter = 0;
function nextSessionId(): string {
  sessionCounter++;
  return `pi_sess_mock_${sessionCounter}`;
}

/**
 * turn_auto handler factory — takes a SessionPool so the pool is shared
 * across requests. The pool.ensure() call either reuses an existing
 * session or creates a new one; if the pool is full, it throws
 * HOT_SESSION_LIMIT_REACHED (-32002) with data.candidate.
 */
export function turnAutoHandlerWithPool(pool: SessionPool): RequestHandler {
  return async (params) => {
    const p = params as TurnAutoParams;
    if (!p.logical_subagent_id || !p.prompt) {
      throw new SidecarError('missing required fields', -32602);
    }
    const { adapter_session_id, reused } = pool.ensure(p.logical_subagent_id, nextSessionId);
    const result: TurnAutoResult = {
      adapter_session_id,
      session_reused: reused,
      status: 'completed',
      result: {
        task_summary: `[mock] turn completed for: ${p.prompt.slice(0, 80)}`,
      },
      usage: {
        model: p.model ?? 'deepseek-chat',
        provider: 'deepseek',
        input_tokens: p.prompt.length,
        output_tokens: 50,
        cache_read_tokens: 0,
        cache_write_tokens: 0,
        cost_usd: 0.001,
      },
    };
    return result;
  };
}
