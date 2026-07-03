import { describe, it, expect, beforeAll } from 'vitest';
import { spawn, execFileSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import * as path from 'node:path';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const SIDECAR_DIR = path.resolve(__dirname, '..');
const BUNDLE = path.join(SIDECAR_DIR, 'dist', 'pi-sidecar.bundle.js');

/**
 * Build the CJS bundle before the smoke test runs. esbuild is fast (~1s) and
 * does not execute the bundled code, so this works even without Pi credentials.
 */
beforeAll(() => {
  execFileSync('node', ['esbuild.config.mjs'], {
    cwd: SIDECAR_DIR,
    stdio: 'pipe',
  });
}, 30000);

interface LineReader {
  lines: string[];
  waitFor: (predicate: (line: string) => boolean, timeoutMs: number) => Promise<string>;
}

/** Line-buffered reader over a child process stdout stream. */
function attachLineReader(stream: NodeJS.ReadableStream): LineReader {
  const lines: string[] = [];
  const waiters: Array<{ predicate: (l: string) => boolean; resolve: (l: string) => void }> = [];
  let buffer = '';
  stream.setEncoding('utf8');
  stream.on('data', (chunk: string) => {
    buffer += chunk;
    let idx: number;
    while ((idx = buffer.indexOf('\n')) >= 0) {
      const line = buffer.slice(0, idx);
      buffer = buffer.slice(idx + 1);
      if (line.trim()) {
        lines.push(line);
        for (const w of [...waiters]) {
          if (w.predicate(line)) {
            const i = waiters.indexOf(w);
            if (i >= 0) waiters.splice(i, 1);
            w.resolve(line);
          }
        }
      }
    }
  });
  return {
    lines,
    waitFor: (predicate, timeoutMs) =>
      new Promise<string>((resolve, reject) => {
        for (const l of lines) {
          if (predicate(l)) {
            resolve(l);
            return;
          }
        }
        const w = { predicate, resolve: (l: string) => resolve(l) };
        waiters.push(w);
        setTimeout(
          () => {
            const i = waiters.indexOf(w);
            if (i >= 0) waiters.splice(i, 1);
            reject(
              new Error(
                `timed out after ${timeoutMs}ms waiting for line; stdout so far:\n${lines.join('\n')}`,
              ),
            );
          },
          timeoutMs,
        );
      }),
  };
}

function rpc(method: string, params: unknown, id: number): string {
  return JSON.stringify({ jsonrpc: '2.0', method, params, id }) + '\n';
}

/**
 * Real handshake smoke test (P2 fix): spawns the CJS bundle and drives a real
 * JSON-RPC exchange over stdio. This proves the bundle boots and speaks the
 * protocol — NOT a `--help`/`--version` check (main.ts has no CLI args).
 *
 * The shutdown response may be dropped if process.exit(0) races the stdout
 * flush, so we treat the `exit code 0` as the authoritative shutdown signal.
 */
describe('CJS bundle JSON-RPC handshake', () => {
  it('boots, answers adapter.initialize, and exits 0 on adapter.shutdown', async () => {
    const child = spawn('node', [BUNDLE], { stdio: ['pipe', 'pipe', 'pipe'] });
    const stderrBuf: string[] = [];
    child.stderr.setEncoding('utf8');
    child.stderr.on('data', (c: string) => {
      stderrBuf.push(c);
    });

    const stdout = attachLineReader(child.stdout!);

    // 1. adapter.initialize handshake.
    child.stdin.write(rpc('adapter.initialize', { protocol_version: 1 }, 1));
    const initLine = await stdout.waitFor((l) => l.includes('"id":1'), 5000);
    const initResp = JSON.parse(initLine) as {
      jsonrpc: string; id: number; result?: { protocol_version: number; sidecar_version: string }; error?: unknown;
    };
    expect(initResp.jsonrpc).toBe('2.0');
    expect(initResp.id).toBe(1);
    expect(initResp.error).toBeUndefined();
    expect(initResp.result?.protocol_version).toBe(1);
    expect(typeof initResp.result?.sidecar_version).toBe('string');

    // 2. adapter.shutdown (triggers ctx.stop → process.exit(0)).
    child.stdin.write(rpc('adapter.shutdown', null, 2));

    // 3. Assert the child exits with code 0 within 5s.
    const exitCode = await new Promise<number>((resolve, reject) => {
      const timer = setTimeout(() => {
        reject(
          new Error(
            `child did not exit within 5s; stderr:\n${stderrBuf.join('')}\nstdout:\n${stdout.lines.join('\n')}`,
          ),
        );
      }, 5000);
      child.on('exit', (code) => {
        clearTimeout(timer);
        resolve(code ?? -1);
      });
    });
    expect(exitCode).toBe(0);

    // Best-effort: if the shutdown response was flushed before exit, validate it.
    const shutdownLine = stdout.lines.find((l) => l.includes('"id":2'));
    if (shutdownLine) {
      const shutdownResp = JSON.parse(shutdownLine) as { id: number; error?: unknown; result?: { ok: boolean } };
      expect(shutdownResp.id).toBe(2);
      expect(shutdownResp.error).toBeUndefined();
      expect(shutdownResp.result?.ok).toBe(true);
    }
  }, 15000);
});
