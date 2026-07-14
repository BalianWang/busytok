import type { RequestHandler } from '../rpc.js';
import type { SessionPool } from '../session_pool.js';
import type { PrepareHibernateParams, PrepareHibernateResult, HibernateSessionEntry } from '../types.js';
import { SidecarError } from '../errors.js';

export function prepareHibernateHandlerWithPool(pool: SessionPool): RequestHandler {
  return async (params) => {
    const p = (params ?? {}) as PrepareHibernateParams;
    // `all: true` — compact all sessions (used by graceful shutdown / idle
    // exit, spec §5.4). Returns a per-session breakdown so the Rust
    // shutdown path can persist each session's memory delta individually.
    if (p.all) {
      const sessions = pool.toArray();
      const entries: HibernateSessionEntry[] = sessions.map((s) => ({
        adapter_session_id: s.adapter_session_id,
        logical_subagent_id: s.logical_subagent_id,
        // Mock memory — Plan 4 (ContextBuilder) wires real memory.
        memory_delta: { hot_summary: `[hibernate-all] session ${s.adapter_session_id} compacted` },
        stats: {},
      }));
      const result: PrepareHibernateResult = {
        stats: { sessions_compacted: entries.length },
        sessions: entries,
      };
      return result;
    }
    // Single session — compact and return memory delta
    if (!p.adapter_session_id) {
      throw new SidecarError('adapter_session_id required (or all:true)', -32602);
    }
    const session = pool.get(p.adapter_session_id);
    if (!session) {
      throw new SidecarError(`session not found: ${p.adapter_session_id}`, -32001);
    }
    if (!pool.reserveForEviction(p.adapter_session_id)) {
      throw new SidecarError(
        `session is busy or unavailable: ${p.adapter_session_id}`,
        -32002,
        { candidate: null, all_busy: true },
      );
    }
    const result: PrepareHibernateResult = {
      memory_delta: {
        hot_summary: `[hibernate] session ${p.adapter_session_id} compacted`,
      },
      stats: { subagent_id: session.logical_subagent_id },
    };
    return result;
  };
}
