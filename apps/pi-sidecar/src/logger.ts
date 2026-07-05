/**
 * Minimal structured logger for the pi-sidecar.
 *
 * The sidecar speaks JSON-RPC over stdout, so log records MUST go to
 * stderr to avoid corrupting the RPC stream. Records are emitted as
 * single-line JSON with a stable shape so downstream collectors can
 * parse them without regex.
 *
 * This helper is intentionally thin — it exists so business code does
 * not scatter bare `console.*` / `process.stderr.write` calls. When a
 * real logger (pino, winston, etc.) is introduced later, only this
 * file needs to change.
 */

export type LogLevel = 'debug' | 'info' | 'warn' | 'error';

export interface LogFields {
  [key: string]: unknown;
}

/**
 * Emit one structured log record to stderr.
 *
 * Shape: `{ ts, level, event, ...fields }`. `ts` is Unix milliseconds.
 * Secrets are the caller's responsibility — never pass `provider_api_key`
 * or other credentials as a field value.
 */
export function logEvent(level: LogLevel, event: string, fields: LogFields = {}): void {
  const record = {
    ts: Date.now(),
    level,
    event,
    ...fields,
  };
  process.stderr.write(JSON.stringify(record) + '\n');
}

/** Convenience wrappers for each level. */
export const logger = {
  debug: (event: string, fields?: LogFields) => logEvent('debug', event, fields),
  info: (event: string, fields?: LogFields) => logEvent('info', event, fields),
  warn: (event: string, fields?: LogFields) => logEvent('warn', event, fields),
  error: (event: string, fields?: LogFields) => logEvent('error', event, fields),
};
