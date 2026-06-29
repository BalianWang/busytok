import { PROTOCOL_VERSION, type InitializeResult } from '../types.js';
import type { RequestHandler } from '../rpc.js';
import { SidecarError } from '../errors.js';

const handler: RequestHandler = async (params) => {
  const p = params as { protocol_version?: number };
  if (p.protocol_version !== PROTOCOL_VERSION) {
    throw new SidecarError(
      `Protocol mismatch: expected ${PROTOCOL_VERSION}, got ${p.protocol_version}`,
      -32008,
    );
  }
  const result: InitializeResult = {
    protocol_version: PROTOCOL_VERSION,
    sidecar_version: '0.1.0',
  };
  return result;
};

export const initializeHandler = handler;
