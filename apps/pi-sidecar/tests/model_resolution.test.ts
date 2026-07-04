import { describe, it, expect, vi, beforeEach } from 'vitest';
import { defaultSessionFactory } from '../src/pi_session.js';

/**
 * Regression + spec coverage for `defaultSessionFactory`.
 *
 * Originally a P0 regression test: "the real SDK path still does not honor
 * the configured model". Now also covers Task 7 spec §5.2: per-session
 * `AuthStorage.inMemory()` + `ModelRegistry.create()` + dynamic
 * `registerProvider()` (no global `cachedRegistry`, no env vars, no file I/O).
 *
 * The SDK module is mocked so we can both (a) capture the `createAgentSession`
 * call args and (b) capture `registerProvider` calls + control what
 * `ModelRegistry.find()` returns.
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
  // Captures `registry.registerProvider(providerId, config)` calls so the
  // spec §5.2 tests can assert on the API shape, baseUrl, and model metadata.
  registerProvider: vi.fn((_providerId: string, _config: unknown) => {
    // Pretend registration succeeded; `find` will return fakeModel for
    // the registered (provider, model) pair.
  }),
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
      registerProvider: hoisted.registerProvider,
    }),
  },
  AuthStorage: {
    inMemory: (_creds?: Record<string, unknown>) => ({}),
  },
}));

/** Common opts for the new (Task 7) required-field shape. */
function makeOpts(overrides: Partial<Parameters<typeof defaultSessionFactory>[1]> = {}) {
  return {
    cwd: '/tmp',
    model: 'test-model-id',
    provider_id: 'test-provider',
    provider_kind: 'openai_compatible' as const,
    provider_base_url: 'https://api.test.com',
    provider_api_key: 'sk-test',
    model_reasoning: false,
    model_context_window: 128000,
    model_max_tokens: 16384,
    ...overrides,
  };
}

describe('defaultSessionFactory model resolution (regression for P0)', () => {
  beforeEach(() => {
    hoisted.createAgentSessionCalls.length = 0;
    hoisted.registerProvider.mockClear();
  });

  it('passes the resolved Model object into createAgentSession (does not ignore configured model)', async () => {
    await defaultSessionFactory('sub-1', makeOpts());

    expect(hoisted.createAgentSessionCalls).toHaveLength(1);
    const callOpts = hoisted.createAgentSessionCalls[0]!;
    // The fix: createAgentSession MUST receive a `model` field that is the
    // resolved Model object (not undefined, not a string). Without the fix,
    // `model` would be absent and the SDK would pick its default.
    expect(callOpts.model).toBeDefined();
    expect(callOpts.model).toBe(hoisted.fakeModel);
    expect(callOpts.cwd).toBe('/tmp');
  });

  it('sources provider_id from profile so ModelRegistry.find() receives it', async () => {
    await defaultSessionFactory('sub-2', makeOpts());

    expect(hoisted.createAgentSessionCalls).toHaveLength(1);
    // If provider_id weren't threaded through, find() would be called with
    // undefined and the test-provider/test-model-id branch wouldn't match,
    // so `model` would be absent from the createAgentSession call.
    const callOpts = hoisted.createAgentSessionCalls[0]!;
    expect(callOpts.model).toBe(hoisted.fakeModel);
  });

  it('throws when model is not found in registry after registerProvider', async () => {
    await expect(
      defaultSessionFactory('sub-missing', makeOpts({ model: 'no-such-model' })),
    ).rejects.toThrow(/model not found in registry/);
    // registerProvider is still called (registration is what populates the
    // registry); the failure is at the subsequent `find()` lookup.
    expect(hoisted.registerProvider).toHaveBeenCalledTimes(1);
    expect(hoisted.createAgentSessionCalls).toHaveLength(0);
  });
});

describe('defaultSessionFactory multi-API provider registration (spec §5.2)', () => {
  beforeEach(() => {
    hoisted.registerProvider.mockClear();
    hoisted.createAgentSessionCalls.length = 0;
  });

  it('registers provider with anthropic-messages api for anthropic_compatible kind', async () => {
    const session = await defaultSessionFactory('sub-1', makeOpts({
      provider_kind: 'anthropic_compatible',
      provider_base_url: 'https://api.anthropic.com',
      provider_api_key: 'sk-ant-test',
      model_reasoning: true,
      model_context_window: 200000,
      model_max_tokens: 8192,
      model_display_name: 'Claude Sonnet 4.5',
    }));
    expect(session).toBeDefined();
    expect(hoisted.registerProvider).toHaveBeenCalledTimes(1);
    expect(hoisted.registerProvider).toHaveBeenCalledWith(
      'test-provider',
      expect.objectContaining({
        api: 'anthropic-messages',
        baseUrl: 'https://api.anthropic.com',
      }),
    );
    // The resolved model object must be passed into createAgentSession.
    expect(hoisted.createAgentSessionCalls).toHaveLength(1);
    expect(hoisted.createAgentSessionCalls[0]!.model).toBeDefined();
  });

  it('registers provider with openai-completions api for openai_compatible kind', async () => {
    await defaultSessionFactory('sub-1', makeOpts({
      provider_kind: 'openai_compatible',
      provider_base_url: 'https://api.openai.com',
      provider_api_key: 'sk-test',
      model_reasoning: false,
      model_context_window: 128000,
      model_max_tokens: 16384,
    }));
    expect(hoisted.registerProvider).toHaveBeenCalledWith(
      'test-provider',
      expect.objectContaining({
        api: 'openai-completions',
        baseUrl: 'https://api.openai.com',
      }),
    );
  });

  it('registers model with contextWindow + maxTokens from CreateSessionOpts', async () => {
    await defaultSessionFactory('sub-1', makeOpts({
      provider_kind: 'openai_compatible',
      provider_base_url: 'https://api.test.com',
      provider_api_key: 'sk-test',
      model_reasoning: true,
      model_context_window: 200000,
      model_max_tokens: 8192,
      model_display_name: 'Test Model',
    }));
    expect(hoisted.registerProvider).toHaveBeenCalledWith(
      'test-provider',
      expect.objectContaining({
        models: expect.arrayContaining([
          expect.objectContaining({
            id: 'test-model-id',
            contextWindow: 200000,
            maxTokens: 8192,
            reasoning: true,
            name: 'Test Model',
          }),
        ]),
      }),
    );
  });
});
