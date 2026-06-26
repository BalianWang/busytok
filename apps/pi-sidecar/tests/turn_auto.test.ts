import { describe, it, expect } from 'vitest';
import { SessionPool } from '../src/session_pool.js';
import { SidecarError } from '../src/errors.js';
import { turnAutoHandlerWithPool } from '../src/handlers/turn_auto.js';

describe('turn_auto with pool', () => {
  it('reuses session for same subagent', async () => {
    const pool = new SessionPool(3);
    const handler = turnAutoHandlerWithPool(pool);
    const params1 = {
      logical_subagent_id: 'sub-a',
      logical_subagent_name: 'a',
      cwd: '/tmp',
      profile: 'pi/search-cheap',
      prompt: 'do 1',
    };
    const result1 = await handler(params1);
    expect(result1.session_reused).toBe(false);

    const params2 = { ...params1, prompt: 'do 2' };
    const result2 = await handler(params2);
    expect(result2.session_reused).toBe(true);
    expect(result2.adapter_session_id).toBe(result1.adapter_session_id);
  });

  it('throws HOT_SESSION_LIMIT_REACHED when full', async () => {
    const pool = new SessionPool(1);
    const handler = turnAutoHandlerWithPool(pool);
    await handler(
      { logical_subagent_id: 'sub-a', cwd: '/tmp', profile: 'p', prompt: 'x' },
    );
    await expect(
      handler(
        { logical_subagent_id: 'sub-b', cwd: '/tmp', profile: 'p', prompt: 'x' },
      ),
    ).rejects.toThrow(SidecarError);
    try {
      await handler(
        { logical_subagent_id: 'sub-b', cwd: '/tmp', profile: 'p', prompt: 'x' },
      );
    } catch (e) {
      expect((e as SidecarError).code).toBe(-32002);
      expect((e as SidecarError).data?.candidate).toBeTruthy();
    }
  });
});
