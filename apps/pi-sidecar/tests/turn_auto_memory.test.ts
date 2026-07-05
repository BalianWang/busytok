import { describe, it, expect, beforeAll, afterAll, afterEach } from 'vitest';
import { SessionPool } from '../src/session_pool.js';
import { turnAutoHandlerWithPool } from '../src/handlers/turn_auto.js';

// These tests exercise the MOCK turn_auto path (BUSYTOK_USE_MOCK_SIDECAR=1)
// to verify memory_update behavior without hitting the real Pi SDK.
const PREV_MOCK_SIDECAR = process.env.BUSYTOK_USE_MOCK_SIDECAR;
beforeAll(() => {
  process.env.BUSYTOK_USE_MOCK_SIDECAR = '1';
});
afterAll(() => {
  if (PREV_MOCK_SIDECAR === undefined) delete process.env.BUSYTOK_USE_MOCK_SIDECAR;
  else process.env.BUSYTOK_USE_MOCK_SIDECAR = PREV_MOCK_SIDECAR;
});

describe('turn_auto memory_update + structured params', () => {
  const origEnv = process.env.BUSYTOK_MOCK_MEMORY_UPDATE;

  afterEach(() => {
    if (origEnv === undefined) delete process.env.BUSYTOK_MOCK_MEMORY_UPDATE;
    else process.env.BUSYTOK_MOCK_MEMORY_UPDATE = origEnv;
  });

  it('emits memory_update when BUSYTOK_MOCK_MEMORY_UPDATE=1', async () => {
    process.env.BUSYTOK_MOCK_MEMORY_UPDATE = '1';
    const pool = new SessionPool(3);
    const handler = turnAutoHandlerWithPool(pool);
    const result = await handler({
      logical_subagent_id: 'sub-a',
      prompt: 'check auth',
      cwd: '/repo',
      profile: 'pi/review-cheap',
      model: 'test-model',
    } as any) as any;
    expect(result.result.memory_update).toBeDefined();
    expect(result.result.memory_update.current_state_summary).toContain('memory update');
    expect(result.result.memory_update.key_files).toHaveLength(1);
    expect(result.result.memory_update.key_files[0].path).toBe('src/auth/token.ts');
    expect(result.result.memory_update.open_questions).toHaveLength(1);
  });

  it('omits memory_update when env unset', async () => {
    delete process.env.BUSYTOK_MOCK_MEMORY_UPDATE;
    const pool = new SessionPool(3);
    const handler = turnAutoHandlerWithPool(pool);
    const result = await handler({
      logical_subagent_id: 'sub-a',
      prompt: 'check auth',
      cwd: '/repo',
      profile: 'pi/review-cheap',
      model: 'test-model',
    } as any) as any;
    expect(result.result.memory_update).toBeUndefined();
  });

  it('accepts structured memory + context params without error', async () => {
    process.env.BUSYTOK_MOCK_MEMORY_UPDATE = '1';
    const pool = new SessionPool(3);
    const handler = turnAutoHandlerWithPool(pool);
    const result = await handler({
      logical_subagent_id: 'sub-a',
      prompt: 'check auth',
      cwd: '/repo',
      profile: 'pi/review-cheap',
      model: 'test-model',
      memory: {
        hot_summary: 'prev state',
        key_files: [{ path: 'src/a.ts', reason: 'r', last_seen_at_ms: 1, score: 1 }],
        decisions: [],
        open_questions: [],
      },
      context: { compact_context: 'full context', budget_tokens: 4000, source: 'busytok-context-builder/v1' },
    } as any) as any;
    expect(result.status).toBe('completed');
    // task_summary is NOT an echo of compact_context (P2-4: no production pollution).
    expect(result.result.task_summary).not.toContain('full context');
  });
});
