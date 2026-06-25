import { type HealthResult } from '../types.js';
import type { RequestHandler } from '../rpc.js';

export const healthHandler: RequestHandler = async () => {
  const result: HealthResult = {
    status: 'healthy',
    sessions: 0,
    rss_mb: Math.round(process.memoryUsage().rss / 1024 / 1024),
  };
  return result;
};
