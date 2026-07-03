import { describe, it, expect, vi, beforeEach } from 'vitest';
import { defaultSessionFactory } from '../src/pi_session.js';

/**
 * Regression test for [P0]: "The real SDK path still does not honor the
 * configured model".
 *
 * Asserts that `defaultSessionFactory` resolves `provider_id` + `model` to a
 * real `Model<any>` object via `ModelRegistry` and passes that object into
 * `createAgentSession({ model })`. Without the fix, the factory passed only
 * `cwd`/`tools` and let the SDK pick its default — silently billing/auditing
 * the wrong model.
 *
 * The SDK module is mocked so we can both (a) capture the `createAgentSession`
 * call args and (b) control what `ModelRegistry.find()` returns. Each test
 * file runs in its own module context, so `cachedRegistry` starts fresh here.
 */
const hoisted = vi.hoisted(() => ({
  createAgentSessionCalls: [] as Array<Record<string, unknown>>,
  // The fake `Model<any>` object that ModelRegistry.find() will resolve to.
  fakeModel: {
    id: 'test-model-id',
    provider: 'test-provider',
    name: 'Test Model',
    api: {},
  },
}));

vi.mock('@earendil-works/pi-coding-agent', () => ({
  createAgentSession: vi.fn(async (opts: Record<string, unknown>) => {
    hoisted.createAgentSessionCalls.push(opts);
    return {
      session: {
        sessionId: 'fake-session-id',
        model: hoisted.fakeModel,
        prompt: async () => {},
        getLastAssistantText: () => 'done',
        getSessionStats: () => ({
          tokens: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 },
          cost: 0,
        }),
        abort: async () => {},
        dispose: () => {},
      },
    };
  }),
  ModelRegistry: {
    create: () => ({
      find: (provider: string, modelId: string) => {
        if (provider === 'test-provider' && modelId === 'test-model-id') {
          return hoisted.fakeModel;
        }
        return undefined;
      },
      getAll: () => [hoisted.fakeModel],
    }),
  },
  AuthStorage: {
    inMemory: () => ({}),
  },
}));

describe('defaultSessionFactory model resolution (regression for P0)', () => {
  beforeEach(() => {
    hoisted.createAgentSessionCalls.length = 0;
  });

  it('passes the resolved Model object into createAgentSession (does not ignore configured model)', async () => {
    await defaultSessionFactory('sub-1', {
      cwd: '/tmp',
      model: 'test-model-id',
      provider_id: 'test-provider',
    });

    expect(hoisted.createAgentSessionCalls).toHaveLength(1);
    const callOpts = hoisted.createAgentSessionCalls[0];
    // The fix: createAgentSession MUST receive a `model` field that is the
    // resolved Model object (not undefined, not a string). Without the fix,
    // `model` would be absent and the SDK would pick its default.
    expect(callOpts.model).toBeDefined();
    expect(callOpts.model).toBe(hoisted.fakeModel);
    expect(callOpts.cwd).toBe('/tmp');
  });

  it('sources provider_id from profile so ModelRegistry.find() receives it', async () => {
    await defaultSessionFactory('sub-2', {
      cwd: '/tmp',
      model: 'test-model-id',
      provider_id: 'test-provider',
    });

    expect(hoisted.createAgentSessionCalls).toHaveLength(1);
    // If provider_id weren't threaded through, find() would be called with
    // undefined and the test-provider/test-model-id branch wouldn't match,
    // so `model` would be absent from the createAgentSession call.
    const callOpts = hoisted.createAgentSessionCalls[0];
    expect(callOpts.model).toBe(hoisted.fakeModel);
  });

  it('omits model when modelId is not provided (SDK picks default)', async () => {
    await defaultSessionFactory('sub-3', {
      cwd: '/tmp',
      // no model, no provider_id
    });

    expect(hoisted.createAgentSessionCalls).toHaveLength(1);
    const callOpts = hoisted.createAgentSessionCalls[0];
    expect(callOpts.model).toBeUndefined();
    expect(callOpts.cwd).toBe('/tmp');
  });
});
