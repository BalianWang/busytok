import { type TurnAutoParams, type TurnAutoResult } from '../types.js';
import type { RequestHandler } from '../rpc.js';
import { SidecarError } from '../errors.js';

// Module-level mock session counter. Wrapped in `nextSessionId()` so the
// mutable state is contained and the intent (generate a fresh mock id) is
// explicit. Plan 3 replaces this with real adapter-managed session ids.
let sessionCounter = 0;
function nextSessionId(): string {
  sessionCounter++;
  return `pi_sess_mock_${sessionCounter}`;
}

export const turnAutoHandler: RequestHandler = async (params) => {
  const p = params as TurnAutoParams;
  if (!p.logical_subagent_id || !p.prompt) {
    throw new SidecarError('missing required fields', -32602);
  }
  const result: TurnAutoResult = {
    adapter_session_id: nextSessionId(),
    session_reused: false,
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
