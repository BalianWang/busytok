import type { CancelParams, CancelResult } from '../types.js';
import type { RequestHandler } from '../rpc.js';
import type { SessionPool } from '../session_pool.js';
import { logger } from '../logger.js';
import { SidecarError } from '../errors.js';

/**
 * `session.cancel` handler — aborts an in-flight `turn_auto` for the given
 * `logical_subagent_id`. This is the execution-protocol counterpart to
 * `SubagentManager::cancel_task`: the manager flips the DB status to
 * `cancelled` and sends a local cancel signal (dropping the executor
 * future), while this RPC actually aborts the underlying SDK HTTP request
 * to the LLM provider — stopping token generation.
 *
 * The session is NOT closed or evicted — it stays in the hot pool and can
 * be reused for subsequent turns. The abort only affects the in-flight
 * `prompt()` call.
 *
 * Best-effort: if no hot session exists for the subagent (turn already
 * completed, or subagent never seen by this sidecar), returns
 * `{ cancelled: false }` without error.
 */
export function cancelHandlerWithPool(pool: SessionPool): RequestHandler {
  return async (params) => {
    const p = (params ?? {}) as CancelParams;
    if (!p.logical_subagent_id) {
      throw new SidecarError('logical_subagent_id required', -32602);
    }
    const cancelled = await pool.abortSession(p.logical_subagent_id);
    logger.info('session.cancel', { logical_subagent_id: p.logical_subagent_id, cancelled });
    const result: CancelResult = { cancelled };
    return result;
  };
}
