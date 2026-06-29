import { describe, it, expect } from 'vitest';
import { PassThrough } from 'node:stream';
import { JsonRpcServer } from '../src/rpc.js';

function createServer(): { server: JsonRpcServer; input: PassThrough; output: PassThrough } {
  const input = new PassThrough();
  const output = new PassThrough();
  const server = new JsonRpcServer(input, output);
  return { server, input, output };
}

function readResponse(output: PassThrough): Promise<unknown> {
  return new Promise((resolve, reject) => {
    let buf = '';
    output.on('data', (chunk: Buffer) => {
      buf += chunk.toString();
      const nl = buf.indexOf('\n');
      if (nl !== -1) {
        const line = buf.slice(0, nl);
        resolve(JSON.parse(line));
      }
    });
    setTimeout(() => reject(new Error('timeout waiting for response')), 3000);
  });
}

describe('JsonRpcServer', () => {
  it('responds to a registered handler', async () => {
    const { server, input, output } = createServer();
    server.registerHandler('test.echo', async (params) => params);
    server.start();

    const respPromise = readResponse(output);
    input.write(JSON.stringify({ jsonrpc: '2.0', method: 'test.echo', params: { msg: 'hello' }, id: 1 }) + '\n');

    const resp = await respPromise as { result: unknown; id: number };
    expect(resp.id).toBe(1);
    expect(resp.result).toEqual({ msg: 'hello' });
    server.stop();
  });

  it('returns method-not-found error for unregistered methods', async () => {
    const { server, input, output } = createServer();
    server.start();

    const respPromise = readResponse(output);
    input.write(JSON.stringify({ jsonrpc: '2.0', method: 'nope', params: {}, id: 2 }) + '\n');

    const resp = await respPromise as { error: { code: number; message: string }; id: number };
    expect(resp.id).toBe(2);
    expect(resp.error.code).toBe(-32601);
    server.stop();
  });

  it('ignores notifications (no id)', async () => {
    const { server, input, output } = createServer();
    server.registerHandler('test.notif', async (params) => params);
    server.start();

    let gotData = false;
    output.on('data', () => { gotData = true; });
    input.write(JSON.stringify({ jsonrpc: '2.0', method: 'test.notif', params: {} }) + '\n');
    await new Promise((r) => setTimeout(r, 100));
    expect(gotData).toBe(false);
    server.stop();
  });

  it('calls ctx.stop() after writing the response', async () => {
    const { server, input, output } = createServer();
    let stopped = false;
    server.onStop(() => { stopped = true; });
    server.registerHandler('test.stop', async (_params, ctx) => {
      ctx.stop();
      return { ok: true };
    });
    server.start();

    const respPromise = readResponse(output);
    input.write(JSON.stringify({ jsonrpc: '2.0', method: 'test.stop', params: {}, id: 3 }) + '\n');

    const resp = await respPromise as { result: unknown; id: number };
    expect(resp.result).toEqual({ ok: true });
    expect(stopped).toBe(true);
  });

  it('returns parse error for malformed JSON', async () => {
    const { server, input, output } = createServer();
    server.start();

    const respPromise = readResponse(output);
    input.write('not json\n');
    const resp = await respPromise as { error: { code: number }; id: number };
    expect(resp.error.code).toBe(-32700);
    server.stop();
  });
});
