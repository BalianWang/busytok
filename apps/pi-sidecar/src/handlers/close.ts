import type { RequestHandler } from '../rpc.js';
import type { SessionPool } from '../session_pool.js';
import type { CloseParams, CloseResult } from '../types.js';
import { SidecarError } from '../errors.js';

export function closeHandlerWithPool(pool: SessionPool): RequestHandler {
  return async (params) => {
    const p = (params ?? {}) as CloseParams;
    if (!p.adapter_session_id) {
      throw new SidecarError('adapter_session_id required', -32602);
    }
    const session = pool.get(p.adapter_session_id);
    if (!session) {
      throw new SidecarError(`session not found: ${p.adapter_session_id}`, -32001);
    }
    pool.close(p.adapter_session_id);
    const result: CloseResult = { ok: true };
    return result;
  };
}
