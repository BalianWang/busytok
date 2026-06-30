//! Subagents monitoring page (Phase 2 — read-only).
//!
//! Renders the `subagent.runtime_status` envelope: a pressure-gate summary,
//! the subagent registry, the recent task history, and the sidecar worker
//! snapshot. The page is strictly read-only — no hibernate/delete/restart
//! controls — matching spec §4 Phase 2. Polling (5s) and envelope handling
//! live in `useSubagentRuntimeStatus`; this component only renders the data.

import { useEffect } from "react";
import type {
  SubagentRuntimeSubagentDto,
  SubagentRuntimeTaskDto,
  SubagentWorkerDto,
} from "@busytok/protocol-types";
import { useSubagentRuntimeStatus } from "../api/useBusytokData";
import { PageState } from "../components/PageState";
import { SettingsRow } from "../components/desktop/SettingsRow";
import { SettingsValue } from "../components/desktop/SettingsValue";
import { SettingsActionGroup } from "../components/desktop/SettingsActionGroup";
import { reportFrontendEventSafely } from "../logging/safeReporter";

const PRESSURE_WARN_LEVELS = new Set(["pressure", "throttled", "limit_exceeded"]);

function pressureTone(level: string): "default" | "warning" {
  return PRESSURE_WARN_LEVELS.has(level) ? "warning" : "default";
}

/** Formats `worker_sampled_at_ms` into a freshness label; `—` if never sampled. */
function formatSampleFreshness(sampledAtMs: number | null): string {
  if (sampledAtMs == null) return "—";
  const secondsAgo = Math.max(0, Math.round((Date.now() - sampledAtMs) / 1000));
  return `sampled ${secondsAgo}s ago`;
}

function formatUptime(seconds: number | null): string {
  if (seconds == null) return "—";
  if (seconds < 60) return `${seconds}s`;
  const m = Math.floor(seconds / 60);
  const s = seconds % 60;
  return `${m}m ${s}s`;
}

function formatTimestamp(ms: number | null): string {
  if (ms == null) return "never";
  return new Date(ms).toLocaleTimeString();
}

export function SubagentsPage() {
  const { data: envelope, isLoading, isError } = useSubagentRuntimeStatus();

  // Telemetry: best-effort page-view signal (non-blocking).
  useEffect(() => {
    reportFrontendEventSafely({
      level: "INFO",
      event_code: "subagent.page_viewed",
      message: "Subagents monitoring page viewed",
    });
  }, []);

  if (isLoading) {
    return (
      <PageState
        kind="loading"
        title="Loading subagents"
        message="Fetching runtime status from the busytok service…"
      />
    );
  }

  if (isError || !envelope) {
    return (
      <PageState
        kind="error"
        title="Couldn't load subagents"
        message="The runtime status RPC failed. Polling will retry automatically."
      />
    );
  }

  const status = envelope.data;
  const { pressure_gate: gate, subagents, tasks_recent, workers } = status;
  const showDegraded = envelope.is_stale || !envelope.is_exact;
  const degradedReason = envelope.degraded_reason ?? null;

  return (
    <div className="settings-page">
      <div className="settings-pane">
        {showDegraded && (
          <div className="degraded-ribbon" role="status">
            <span className="degraded-ribbon__dot" aria-hidden="true" />
            <span>
              {degradedReason ??
                (envelope.is_stale
                  ? "Showing stale data — refresh in progress"
                  : "Data is approximate — exact aggregates not yet available")}
            </span>
          </div>
        )}

        {/* 1. Pressure-gate summary */}
        <section className="settings-section">
          <h2>Pressure Summary</h2>
          <div className="settings-panel">
            <SettingsRow
              label="Pressure level"
              description="Resource-pressure state of the runtime"
              control={
                <SettingsValue
                  value={gate.level}
                  tone={pressureTone(gate.level)}
                />
              }
            />
            <SettingsRow
              label="Memory used"
              description="Resident-set pressure across managed processes"
              control={<SettingsValue value={`${gate.memory_used_pct}%`} />}
            />
            <SettingsRow
              label="Hot sessions"
              description="Active hot in-memory contexts vs. limit"
              control={
                <SettingsValue
                  value={`${gate.hot_sessions_total} / ${gate.hot_sessions_limit}`}
                  tone={
                    gate.hot_sessions_total >= gate.hot_sessions_limit
                      ? "warning"
                      : "default"
                  }
                />
              }
            />
            <SettingsRow
              label="Sample freshness"
              description="When the worker resource sample was last taken"
              control={
                <SettingsValue
                  value={formatSampleFreshness(gate.worker_sampled_at_ms)}
                  tone="muted"
                />
              }
            />
          </div>
        </section>

        {/* 2. Subagent registry */}
        <section className="settings-section">
          <h2>Subagents</h2>
          <div className="settings-panel">
            {subagents.length === 0 ? (
              <SettingsRow
                label="No subagents"
                description="No subagent rows in the registry"
                control={<SettingsValue value="none" tone="muted" />}
              />
            ) : (
              subagents.map((s: SubagentRuntimeSubagentDto) => (
                <SettingsRow
                  key={s.name}
                  label={s.name}
                  description={`Last task: ${formatTimestamp(s.last_task_at_ms)}${
                    s.last_task_status ? ` (${s.last_task_status})` : ""
                  }`}
                  control={
                    <SettingsActionGroup direction="row">
                      <SettingsValue value={s.status} />
                      <SettingsValue value={`${s.task_count} tasks`} tone="muted" />
                    </SettingsActionGroup>
                  }
                />
              ))
            )}
          </div>
        </section>

        {/* 3. Recent task history (cross-all-subagents, last 20) */}
        <section className="settings-section">
          <h2>Task History</h2>
          <div className="settings-panel">
            {tasks_recent.length === 0 ? (
              <SettingsRow
                label="No tasks"
                description="No recent tasks across all subagents"
                control={<SettingsValue value="none" tone="muted" />}
              />
            ) : (
              tasks_recent.map((t: SubagentRuntimeTaskDto) => (
                <SettingsRow
                  key={t.task_id}
                  label={t.task_id}
                  description={`${formatTimestamp(t.created_at_ms)}${
                    t.error ? ` • ${t.error}` : ""
                  }`}
                  control={
                    <SettingsActionGroup direction="row">
                      <SettingsValue value={t.subagent_name} tone="muted" />
                      <SettingsValue
                        value={t.status}
                        tone={t.status === "failed" ? "danger" : "default"}
                      />
                    </SettingsActionGroup>
                  }
                />
              ))
            )}
          </div>
        </section>

        {/* 4. Sidecar workers */}
        <section className="settings-section">
          <h2>Sidecar Workers</h2>
          <div className="settings-panel">
            {workers.length === 0 ? (
              <SettingsRow
                label="No sidecar configured"
                description="No Pi sidecar supervisor is registered"
                control={<SettingsValue value="none" tone="muted" />}
              />
            ) : (
              workers.map((w: SubagentWorkerDto, i: number) => (
                <SettingsRow
                  key={i}
                  label="Sidecar worker"
                  description={`PID: ${w.pid ?? "—"} • Uptime: ${formatUptime(
                    w.uptime_seconds,
                  )} • Hot sessions: ${w.hot_sessions}`}
                  control={
                    <SettingsValue
                      value={w.state}
                      tone={w.state === "running" ? "default" : "muted"}
                    />
                  }
                />
              ))
            )}
          </div>
        </section>
      </div>
    </div>
  );
}
