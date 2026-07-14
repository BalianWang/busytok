import type { RequestHandler } from '../rpc.js';
import type { SessionPool } from '../session_pool.js';
import type { ActivateParams, ActivateResult } from '../types.js';

/**
 * `session.activate` handler — moves a session from `pending` to `active`
 * (LRU-eligible). Called by Rust AFTER the DB hot binding is committed.
 *
 * This closes the timing window between `turn_auto` returning success
 * (session becomes idle) and Rust committing the DB binding. Without
 * activation, a newly-created session stays in `pending` state and cannot
 * be selected as an eviction candidate — preventing the race where a
 * concurrent delegate evicts a session whose binding hasn't been committed
 * yet (the "ghost session" bug).
 *
 * Idempotent: activating an already-active session is a no-op.
 */
export function activateHandlerWithPool(pool: SessionPool): RequestHandler {
  return async (params) => {
    const p = (params ?? {}) as ActivateParams;
    if (!p.adapter_session_id) {
      throw new Error('adapter_session_id required');
    }
    pool.activate(p.adapter_session_id);
    const result: ActivateResult = { ok: true };
    return result;
  };
}
