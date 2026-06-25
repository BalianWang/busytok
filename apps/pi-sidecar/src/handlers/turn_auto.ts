import { type TurnAutoParams, type TurnAutoResult } from '../types.js';
import type { RequestHandler } from '../rpc.js';

let sessionCounter = 0;

export const turnAutoHandler: RequestHandler = async (params) => {
  const p = params as TurnAutoParams;
  if (!p.logical_subagent_id || !p.prompt) {
    const err = new Error('missing required fields');
    (err as unknown as { code: number }).code = -32602;
    throw err;
  }
  sessionCounter++;
  const result: TurnAutoResult = {
    adapter_session_id: `pi_sess_mock_${sessionCounter}`,
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
