import { PROTOCOL_VERSION, type InitializeResult } from '../types.js';
import type { RequestHandler } from '../rpc.js';

const handler: RequestHandler = async (params) => {
  const p = params as { protocol_version?: number };
  if (p.protocol_version !== PROTOCOL_VERSION) {
    const err = new Error(`Protocol mismatch: expected ${PROTOCOL_VERSION}, got ${p.protocol_version}`);
    (err as unknown as { code: number }).code = -32008;
    throw err;
  }
  const result: InitializeResult = {
    protocol_version: PROTOCOL_VERSION,
    sidecar_version: '0.1.0',
  };
  return result;
};

export const initializeHandler = handler;
