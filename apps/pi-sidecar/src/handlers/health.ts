import { type HealthResult } from '../types.js';
import type { RequestHandler } from '../rpc.js';
import type { SessionPool } from '../session_pool.js';

export function healthHandlerWithPool(pool: SessionPool): RequestHandler {
  return async () => {
    const result: HealthResult = {
      status: 'healthy',
      sessions: pool.size(),
      rss_mb: Math.round(process.memoryUsage().rss / 1024 / 1024),
    };
    return result;
  };
}
