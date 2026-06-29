/**
 * Error with an attached JSON-RPC error code.
 *
 * Thrown by handlers when they need to surface a specific JSON-RPC error code
 * (e.g. `-32008` protocol mismatch, `-32602` invalid params). The RPC layer
 * checks `instanceof SidecarError` and propagates `code` into the response;
 * any other thrown value is reported as the default `-32603` (internal error).
 *
 * An optional `data` field is propagated into the JSON-RPC error object when
 * present (e.g. `{ candidate }` for HOT_SESSION_LIMIT_REACHED, spec §5.2).
 */
export class SidecarError extends Error {
  readonly data?: unknown;

  constructor(
    message: string,
    readonly code: number,
    data?: unknown,
  ) {
    super(message);
    this.name = 'SidecarError';
    this.data = data;
  }
}
