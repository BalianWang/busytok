import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { logEvent, logger, type LogLevel } from '../src/logger.js';

describe('logger', () => {
  let writeSpy: ReturnType<typeof vi.spyOn>;
  let stderrBuffer: string[];

  beforeEach(() => {
    stderrBuffer = [];
    writeSpy = vi.spyOn(process.stderr, 'write').mockImplementation((chunk: any) => {
      stderrBuffer.push(String(chunk));
      return true;
    });
  });

  afterEach(() => {
    writeSpy.mockRestore();
  });

  function lastRecord(): Record<string, unknown> {
    expect(stderrBuffer.length).toBeGreaterThan(0);
    const line = stderrBuffer[stderrBuffer.length - 1].trimEnd();
    return JSON.parse(line);
  }

  it('logEvent writes a single-line JSON record to stderr', () => {
    logEvent('debug', 'test.event', { foo: 'bar', count: 3 });
    expect(stderrBuffer.length).toBe(1);
    expect(stderrBuffer[0].endsWith('\n')).toBe(true);
    const rec = lastRecord();
    expect(rec['level']).toBe('debug');
    expect(rec['event']).toBe('test.event');
    expect(rec['foo']).toBe('bar');
    expect(rec['count']).toBe(3);
  });

  it('record has ts (number), level, event fields', () => {
    const before = Date.now();
    logEvent('info', 'test.ts_field');
    const after = Date.now();
    const rec = lastRecord();
    expect(typeof rec['ts']).toBe('number');
    const ts = rec['ts'] as number;
    expect(ts).toBeGreaterThanOrEqual(before);
    expect(ts).toBeLessThanOrEqual(after);
  });

  it('defaults fields to empty object when omitted', () => {
    logEvent('warn', 'test.no_fields');
    const rec = lastRecord();
    expect(rec['level']).toBe('warn');
    expect(rec['event']).toBe('test.no_fields');
    // Only ts, level, event keys present.
    expect(Object.keys(rec).sort()).toEqual(['event', 'level', 'ts']);
  });

  it('does not write to stdout (would corrupt JSON-RPC stream)', () => {
    const stdoutSpy = vi.spyOn(process.stdout, 'write').mockImplementation(() => true);
    try {
      logEvent('error', 'test.no_stdout');
      expect(stdoutSpy).not.toHaveBeenCalled();
      expect(stderrBuffer.length).toBe(1);
    } finally {
      stdoutSpy.mockRestore();
    }
  });

  it.each<[LogLevel]>([['debug'], ['info'], ['warn'], ['error']])(
    'logger.%s wraps logEvent with the correct level',
    (level) => {
      logger[level]('test.wrapper', { n: 1 });
      const rec = lastRecord();
      expect(rec['level']).toBe(level);
      expect(rec['event']).toBe('test.wrapper');
      expect(rec['n']).toBe(1);
    },
  );

  it('logger.debug accepts omitted fields arg', () => {
    logger.debug('test.optional_fields');
    const rec = lastRecord();
    expect(rec['level']).toBe('debug');
    expect(Object.keys(rec).sort()).toEqual(['event', 'level', 'ts']);
  });

  it('does not mutate the caller-supplied fields object', () => {
    const fields = { logical_subagent_id: 'sub-a', count: 2 };
    logEvent('info', 'test.immutable', fields);
    expect(fields).toEqual({ logical_subagent_id: 'sub-a', count: 2 });
  });
});
