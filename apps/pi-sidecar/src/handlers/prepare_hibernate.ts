import type { RequestHandler } from '../rpc.js';

export const prepareHibernateHandler: RequestHandler = async () => {
  // Plan 3: wire to real session pool
  return { memory_delta: null, stats: {} };
};
