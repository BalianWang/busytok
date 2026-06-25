import { describe, it, expect } from 'vitest';
import { initializeHandler } from '../src/handlers/initialize.js';
import { healthHandler } from '../src/handlers/health.js';
import { shutdownHandler } from '../src/handlers/shutdown.js';
import { turnAutoHandler } from '../src/handlers/turn_auto.js';
import { prepareHibernateHandler } from '../src/handlers/prepare_hibernate.js';
import { closeHandler } from '../src/handlers/close.js';
import type { HandlerContext } from '../src/rpc.js';

const noopCtx: HandlerContext = { stop: () => {} };

describe('initialize handler', () => {
  it('returns protocol version on match', async () => {
    const result = await initializeHandler({ protocol_version: 1 }, noopCtx) as {
      protocol_version: number; sidecar_version: string;
    };
    expect(result.protocol_version).toBe(1);
    expect(result.sidecar_version).toBe('0.1.0');
  });

  it('throws PROTOCOL_MISMATCH on version mismatch', async () => {
    await expect(initializeHandler({ protocol_version: 99 }, noopCtx)).rejects.toThrow();
    try {
      await initializeHandler({ protocol_version: 99 }, noopCtx);
    } catch (e) {
      expect((e as { code: number }).code).toBe(-32008);
    }
  });
});

describe('health handler', () => {
  it('returns healthy status', async () => {
    const result = await healthHandler({}, noopCtx) as {
      status: string; sessions: number; rss_mb: number;
    };
    expect(result.status).toBe('healthy');
    expect(result.sessions).toBe(0);
    expect(result.rss_mb).toBeGreaterThan(0);
  });
});

describe('shutdown handler', () => {
  it('calls ctx.stop() and returns ok', async () => {
    let stopped = false;
    const ctx: HandlerContext = { stop: () => { stopped = true; } };
    const result = await shutdownHandler({}, ctx) as { ok: boolean };
    expect(result.ok).toBe(true);
    expect(stopped).toBe(true);
  });
});

describe('turn_auto handler', () => {
  it('returns completed result with usage', async () => {
    const result = await turnAutoHandler(
      { logical_subagent_id: 'sa_1', prompt: 'do the thing', cwd: '/tmp', profile: 'pi/default' },
      noopCtx,
    ) as {
      adapter_session_id: string; status: string;
      result: { task_summary: string };
      usage: { input_tokens: number; output_tokens: number };
    };
    expect(result.status).toBe('completed');
    expect(result.adapter_session_id).toMatch(/^pi_sess_mock_/);
    expect(result.usage.input_tokens).toBe('do the thing'.length);
    expect(result.usage.output_tokens).toBe(50);
  });

  it('throws on missing required fields', async () => {
    await expect(turnAutoHandler({ cwd: '/tmp' }, noopCtx)).rejects.toThrow();
    try {
      await turnAutoHandler({ cwd: '/tmp' }, noopCtx);
    } catch (e) {
      expect((e as { code: number }).code).toBe(-32602);
    }
  });
});

describe('prepare_hibernate handler', () => {
  it('returns stub response', async () => {
    const result = await prepareHibernateHandler({}, noopCtx) as {
      memory_delta: unknown; stats: Record<string, unknown>;
    };
    expect(result.memory_delta).toBeNull();
    expect(result.stats).toEqual({});
  });
});

describe('close handler', () => {
  it('returns ok', async () => {
    const result = await closeHandler({}, noopCtx) as { ok: boolean };
    expect(result.ok).toBe(true);
  });
});
