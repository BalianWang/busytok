import { describe, it, expect, beforeAll, afterAll } from 'vitest';
import { SessionPool } from '../src/session_pool.js';
import { SidecarError } from '../src/errors.js';
import { turnAutoHandlerWithPool } from '../src/handlers/turn_auto.js';

// These tests exercise the MOCK turn_auto path (BUSYTOK_USE_MOCK_SIDECAR=1),
// which keeps the hardcoded mock usage so Rust-side e2e tests need no real
// Pi credentials. The real SDK path is covered by real_sdk.test.ts.
const PREV_MOCK = process.env.BUSYTOK_USE_MOCK_SIDECAR;
beforeAll(() => {
  process.env.BUSYTOK_USE_MOCK_SIDECAR = '1';
});
afterAll(() => {
  if (PREV_MOCK === undefined) delete process.env.BUSYTOK_USE_MOCK_SIDECAR;
  else process.env.BUSYTOK_USE_MOCK_SIDECAR = PREV_MOCK;
});

describe('turn_auto with pool (mock path)', () => {
  it('reuses session for same subagent', async () => {
    const pool = new SessionPool(3);
    const handler = turnAutoHandlerWithPool(pool);
    const params1 = {
      logical_subagent_id: 'sub-a',
      logical_subagent_name: 'a',
      cwd: '/tmp',
      profile: 'pi/search-cheap',
      model: 'test-model',
      prompt: 'do 1',
    };
    const result1 = await handler(params1) as { session_reused: boolean; adapter_session_id: string };
    expect(result1.session_reused).toBe(false);
    expect(result1.adapter_session_id).toMatch(/^pi_sess_mock_/);

    const params2 = { ...params1, prompt: 'do 2' };
    const result2 = await handler(params2) as { session_reused: boolean; adapter_session_id: string };
    expect(result2.session_reused).toBe(true);
    expect(result2.adapter_session_id).toBe(result1.adapter_session_id);
  });

  it('throws HOT_SESSION_LIMIT_REACHED when full', async () => {
    const pool = new SessionPool(1);
    const handler = turnAutoHandlerWithPool(pool);
    const result1 = await handler(
      { logical_subagent_id: 'sub-a', cwd: '/tmp', profile: 'p', model: 'test-model', prompt: 'x' },
    ) as { adapter_session_id: string };
    // Two-phase lifecycle: activate the session so it becomes an evictable
    // LRU candidate (simulates Rust calling session.activate after DB commit).
    pool.activate(result1.adapter_session_id);
    await expect(
      handler(
        { logical_subagent_id: 'sub-b', cwd: '/tmp', profile: 'p', model: 'test-model', prompt: 'x' },
      ),
    ).rejects.toThrow(SidecarError);
    try {
      await handler(
        { logical_subagent_id: 'sub-b', cwd: '/tmp', profile: 'p', model: 'test-model', prompt: 'x' },
      );
    } catch (e) {
      expect((e as SidecarError).code).toBe(-32002);
      expect((e as SidecarError).data?.candidate).toBeTruthy();
    }
  });
});
