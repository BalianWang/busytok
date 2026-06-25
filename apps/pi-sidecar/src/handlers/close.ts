import type { RequestHandler } from '../rpc.js';

export const closeHandler: RequestHandler = async () => {
  // Plan 3: wire to real session pool
  return { ok: true };
};
