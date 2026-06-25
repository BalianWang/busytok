/**
 * Error with an attached JSON-RPC error code.
 *
 * Thrown by handlers when they need to surface a specific JSON-RPC error code
 * (e.g. `-32008` protocol mismatch, `-32602` invalid params). The RPC layer
 * checks `instanceof SidecarError` and propagates `code` into the response;
 * any other thrown value is reported as the default `-32603` (internal error).
 */
export class SidecarError extends Error {
  constructor(
    message: string,
    readonly code: number,
  ) {
    super(message);
    this.name = 'SidecarError';
  }
}
