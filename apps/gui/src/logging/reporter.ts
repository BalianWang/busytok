// apps/gui/src/logging/reporter.ts

import { invoke } from "@tauri-apps/api/core";

// ── Types ────────────────────────────────────────────────────────

interface FrontendLogEntry {
  id: string;
  ts: string;
  level: "ERROR" | "WARN" | "INFO";
  source: "frontend";
  session_id: string;
  correlation_id?: string;
  event_code: string;
  message: string;
  details?: Record<string, unknown>;
}

// ── Constants ────────────────────────────────────────────────────

const BUFFER_KEY = "busytok_frontend_log_buffer";
const MAX_BUFFER_SIZE = 50;
const MAX_DETAILS_SIZE = 16 * 1024; // 16KB
const DEDUP_WINDOW_MS = 5000;

// ── State ────────────────────────────────────────────────────────

let sessionId: string | null = null;
let flushInFlight = false;
const seenErrors = new Map<string, number>(); // key -> timestamp_ms

type PanelBridge = {
  invoke(method: string, payload?: unknown): Promise<unknown>;
};

function panelBridge(): PanelBridge | null {
  const bridge = (globalThis as typeof globalThis & {
    window?: Window & { busytokPanelBridge?: PanelBridge };
  }).window?.busytokPanelBridge;
  return bridge ?? null;
}

function invokeHost(command: string, args?: Record<string, unknown>): Promise<unknown> {
  const bridge = panelBridge();
  if (bridge) {
    return bridge.invoke(command, args);
  }
  return invoke(command, args);
}

// ── session_id ───────────────────────────────────────────────────

export function getSessionId(): string {
  if (!sessionId) {
    sessionId = crypto.randomUUID();
  }
  return sessionId;
}

// ── Dedup ────────────────────────────────────────────────────────

function dedupKey(event_code: string, message: string, stack?: string): string {
  const topLine = stack?.split("\n")[0] ?? "";
  return `${event_code}|${message}|${topLine}`;
}

function isDuplicate(key: string): boolean {
  const now = Date.now();
  const last = seenErrors.get(key);
  if (last != null && now - last < DEDUP_WINDOW_MS) {
    return true;
  }
  seenErrors.set(key, now);
  // Prune expired entries
  seenErrors.forEach((t, k) => {
    if (now - t > DEDUP_WINDOW_MS) seenErrors.delete(k);
  });
  return false;
}

// ── Details serialization ─────────────────────────────────────────

function serializeDetails(details: unknown): Record<string, unknown> | undefined {
  if (details == null) return undefined;
  try {
    const json = JSON.stringify(details);
    if (json.length > MAX_DETAILS_SIZE) {
      return {
        details_truncated: true,
        _original_size: json.length,
        preview: json.slice(0, MAX_DETAILS_SIZE),
      };
    }
    return JSON.parse(json) as Record<string, unknown>;
  } catch {
    return { _serialization_error: String(details) };
  }
}

// ── Buffer I/O ───────────────────────────────────────────────────

function readBuffer(): FrontendLogEntry[] {
  try {
    const raw = localStorage.getItem(BUFFER_KEY);
    if (!raw) return [];
    const parsed = JSON.parse(raw);
    if (
      typeof parsed !== "object" ||
      parsed == null ||
      !Array.isArray(parsed.entries)
    ) {
      localStorage.removeItem(BUFFER_KEY);
      return [];
    }
    return parsed.entries.filter(
      (e: unknown) =>
        typeof e === "object" &&
        e != null &&
        typeof (e as FrontendLogEntry).event_code === "string",
    );
  } catch {
    localStorage.removeItem(BUFFER_KEY);
    return [];
  }
}

function writeBuffer(entries: FrontendLogEntry[]): void {
  if (entries.length === 0) {
    localStorage.removeItem(BUFFER_KEY);
    return;
  }
  try {
    localStorage.setItem(BUFFER_KEY, JSON.stringify({ entries }));
  } catch {
    // localStorage full — keep only the 10 newest and retry once
    if (entries.length > 10) {
      try {
        localStorage.setItem(
          BUFFER_KEY,
          JSON.stringify({ entries: entries.slice(-10) }),
        );
      } catch {
        // give up
      }
    }
  }
}

function appendToBuffer(entry: FrontendLogEntry): void {
  const entries = [...readBuffer(), entry].slice(-MAX_BUFFER_SIZE);
  writeBuffer(entries);
}

function removeFromBufferById(id: string): void {
  writeBuffer(readBuffer().filter((e) => e.id !== id));
}

// ── Core reporter ────────────────────────────────────────────────

export function reportFrontendEvent(entry: {
  level: "ERROR" | "WARN" | "INFO";
  correlation_id?: string;
  event_code: string;
  message: string;
  details?: Record<string, unknown>;
}): void {
  const log: FrontendLogEntry = {
    id: crypto.randomUUID(),
    ts: new Date().toISOString(),
    level: entry.level,
    source: "frontend",
    session_id: getSessionId(),
    correlation_id: entry.correlation_id,
    event_code: entry.event_code,
    message: entry.message,
    details: serializeDetails(entry.details),
  };

  // Dedup errors within 5s window (key: event_code + message + top stack line)
  if (entry.level === "ERROR") {
    try {
      const stackStr =
        typeof entry.details?.stack === "string"
          ? entry.details.stack
          : typeof entry.details?.reason_stack === "string"
          ? entry.details.reason_stack
          : "";
      const key = dedupKey(entry.event_code, entry.message, stackStr);
      if (isDuplicate(key)) return;
    } catch {
      // Dedup failure is non-fatal; still emit the log entry
    }
  }

  // Buffer first (durability — survives page crash before invoke completes),
  // then best-effort async upload. Remove from buffer on success by stable ID
  // to avoid duplicate re-sends on the next flushBuffer cycle.
  appendToBuffer(log);
  invokeHost("log_frontend_event", { entry: log }).then(() => {
    removeFromBufferById(log.id);
  }).catch(() => {
    // Buffer fallback already in place; invoke failure is non-fatal
  });
}

export function reportFrontendError(entry: {
  correlation_id?: string;
  event_code: string;
  message: string;
  details?: Record<string, unknown>;
}): void {
  reportFrontendEvent({ ...entry, level: "ERROR" });
}

/**
 * Fire-and-forget INFO event that never throws into the caller's path.
 *
 * Use for observability emitted from user-action codepaths (pagination,
 * heatmap model build, etc.) where a logging failure must not break the
 * UI. This is the canonical safe wrapper; domain call sites should use it
 * directly rather than per-feature duplicates.
 */
export function safeReportEvent(
  event_code: string,
  message: string,
  details?: Record<string, unknown>,
): void {
  try {
    reportFrontendEvent({ level: "INFO", event_code, message, details });
  } catch {
    // Observability must not break the user action path.
  }
}

// ── Flush ────────────────────────────────────────────────────────

export async function flushBuffer(): Promise<void> {
  if (flushInFlight) return;
  flushInFlight = true;
  try {
    const entries = readBuffer();
    if (entries.length === 0) return;

    // Filter out locally-invalid entries before sending (empty event_code/message).
    // This mirrors the Rust-side validation in flush_frontend_logs_inner().
    const valid = entries.filter(
      (e) => e.event_code.length > 0 && e.message.length > 0,
    );
    const invalidCount = entries.length - valid.length;
    if (invalidCount > 0) {
      console.warn(`flushBuffer: dropping ${invalidCount} invalid entries`);
    }

    if (valid.length === 0) {
      // All entries were invalid — clear the buffer and stop
      localStorage.removeItem(BUFFER_KEY);
      return;
    }

    const sentSet = new Set(valid.map((e) => `${e.ts}|${e.event_code}|${e.message}`));

    await invokeHost("flush_frontend_logs", {
      entries: valid,
    });

    // Remove sent entries + locally-filtered invalid entries, preserving
    // only new entries added during the async invoke
    const invalidKeys = new Set(
      entries
        .filter((e) => !(e.event_code.length > 0 && e.message.length > 0))
        .map((e) => `${e.ts}|${e.event_code}|${e.message}`),
    );
    const after = readBuffer();
    const remaining = after.filter(
      (e) => !sentSet.has(`${e.ts}|${e.event_code}|${e.message}`)
          && !invalidKeys.has(`${e.ts}|${e.event_code}|${e.message}`),
    );
    writeBuffer(remaining);
  } catch {
    // Buffer persists for next retry
  } finally {
    flushInFlight = false;
  }
}

export function hasBufferedLogs(): boolean {
  return readBuffer().length > 0;
}
