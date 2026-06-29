import type { RequestHandler } from '../rpc.js';

export const shutdownHandler: RequestHandler = async (_params, ctx) => {
  ctx.stop();
  return { ok: true };
};
