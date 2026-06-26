import type { JsonRpcRequest, JsonRpcResponse, JsonRpcError } from './types.js';
import { SidecarError } from './errors.js';
import * as readline from 'node:readline';

export interface HandlerContext {
  /** Signal the server to stop after sending the response. */
  stop: () => void;
}

export type RequestHandler = (
  params: unknown,
  ctx: HandlerContext,
) => Promise<unknown>;

export class JsonRpcServer {
  private rl: readline.Interface;
  private handlers = new Map<string, RequestHandler>();
  private stopCallbacks: Array<() => void> = [];
  private stopped = false;

  constructor(
    private input: NodeJS.ReadableStream = process.stdin,
    private output: NodeJS.WritableStream = process.stdout,
  ) {
    this.rl = readline.createInterface({ input, terminal: false });
  }

  registerHandler(method: string, handler: RequestHandler): void {
    this.handlers.set(method, handler);
  }

  /** Register a callback fired after `stop()` completes (e.g. `process.exit(0)`). */
  onStop(cb: () => void): void {
    this.stopCallbacks.push(cb);
  }

  start(): void {
    this.rl.on('line', (line: string) => {
      this.handleLine(line).catch((err: unknown) => {
        process.stderr.write(`Error handling line: ${err}\n`);
      });
    });
  }

  /** Close the readline interface and fire stop callbacks. Safe to call once. */
  stop(): void {
    if (this.stopped) return;
    this.stopped = true;
    this.rl.close();
    for (const cb of this.stopCallbacks) {
      try {
        cb();
      } catch (err: unknown) {
        process.stderr.write(`Stop callback error: ${err}\n`);
      }
    }
  }

  private async handleLine(line: string): Promise<void> {
    let req: JsonRpcRequest;
    try {
      req = JSON.parse(line);
    } catch {
      if (line.trim()) {
        this.writeError(0, -32700, 'Parse error');
      }
      return;
    }
    if (req.id === undefined) {
      // Notification — no response
      return;
    }
    const handler = this.handlers.get(req.method);
    if (!handler) {
      this.writeError(req.id, -32601, `Method not found: ${req.method}`);
      return;
    }
    let shouldStop = false;
    const ctx: HandlerContext = {
      stop: () => {
        shouldStop = true;
      },
    };
    try {
      const result = await handler(req.params, ctx);
      this.writeResponse(req.id, result);
    } catch (err: unknown) {
      // SidecarError carries a specific JSON-RPC code (and optional data);
      // anything else is surfaced as the default -32603 (internal error).
      if (err instanceof SidecarError) {
        this.writeError(req.id, err.code, err.message, err.data);
      } else {
        const message = err instanceof Error ? err.message : String(err);
        this.writeError(req.id, -32603, message);
      }
    }
    // Stop AFTER the response is written — no race.
    if (shouldStop) {
      this.stop();
    }
  }

  private writeResponse(id: number, result: unknown): void {
    const resp: JsonRpcResponse = { jsonrpc: '2.0', result, id };
    this.output.write(JSON.stringify(resp) + '\n');
  }

  private writeError(id: number, code: number, message: string, data?: unknown): void {
    const err: JsonRpcError = { code, message, ...(data !== undefined ? { data } : {}) };
    const resp: JsonRpcResponse = { jsonrpc: '2.0', error: err, id };
    this.output.write(JSON.stringify(resp) + '\n');
  }
}
