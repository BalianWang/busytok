import { describe, it, expect } from 'vitest';
import { SessionPool } from '../src/session_pool.js';
import { SidecarError } from '../src/errors.js';

describe('SessionPool', () => {
  it('ensure creates new session when under limit', () => {
    const pool = new SessionPool(3);
    const result = pool.ensure('sub-a', () => 'sess-1');
    expect(result.adapter_session_id).toBe('sess-1');
    expect(result.reused).toBe(false);
    expect(pool.size()).toBe(1);
  });

  it('ensure reuses existing session for same subagent', () => {
    const pool = new SessionPool(3);
    pool.ensure('sub-a', () => 'sess-1');
    const result = pool.ensure('sub-a', () => 'sess-2');
    expect(result.adapter_session_id).toBe('sess-1');
    expect(result.reused).toBe(true);
    expect(pool.size()).toBe(1);
  });

  it('ensure throws HOT_SESSION_LIMIT_REACHED with candidate when full', () => {
    const pool = new SessionPool(2);
    pool.ensure('sub-a', () => 'sess-1'); // LRU after next
    pool.ensure('sub-b', () => 'sess-2'); // MRU
    expect(() => pool.ensure('sub-c', () => 'sess-3')).toThrow(SidecarError);
    try {
      pool.ensure('sub-c', () => 'sess-3');
    } catch (e) {
      expect((e as SidecarError).code).toBe(-32002);
      expect((e as SidecarError).data).toEqual({ candidate: 'sess-1' });
    }
  });

  it('close removes session from pool', () => {
    const pool = new SessionPool(3);
    pool.ensure('sub-a', () => 'sess-1');
    pool.close('sess-1');
    expect(pool.size()).toBe(0);
    // Re-ensure creates a new session
    const result = pool.ensure('sub-a', () => 'sess-2');
    expect(result.adapter_session_id).toBe('sess-2');
    expect(result.reused).toBe(false);
  });

  it('LRU order updates on reuse', () => {
    const pool = new SessionPool(2);
    pool.ensure('sub-a', () => 'sess-1'); // LRU
    pool.ensure('sub-b', () => 'sess-2'); // MRU
    // Reuse sess-1 → it becomes MRU, sess-2 becomes LRU
    pool.ensure('sub-a', () => 'sess-1');
    expect(() => pool.ensure('sub-c', () => 'sess-3')).toThrow(SidecarError);
    try {
      pool.ensure('sub-c', () => 'sess-3');
    } catch (e) {
      expect((e as SidecarError).data).toEqual({ candidate: 'sess-2' });
    }
  });

  it('get returns session by adapter_session_id', () => {
    const pool = new SessionPool(3);
    pool.ensure('sub-a', () => 'sess-1');
    const session = pool.get('sess-1');
    expect(session).toBeDefined();
    expect(session!.logical_subagent_id).toBe('sub-a');
  });

  it('get returns undefined for unknown session', () => {
    const pool = new SessionPool(3);
    expect(pool.get('unknown')).toBeUndefined();
  });
});
