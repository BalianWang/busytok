import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, cleanup } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type {
  ReadEnvelopeDto,
  SubagentRuntimeStatusDto,
  SubagentRuntimeSubagentDto,
  SubagentRuntimeTaskDto,
  SubagentWorkerDto,
  SubagentPressureGateDto,
} from "@busytok/protocol-types";

vi.mock("../api/useBusytokData", () => ({
  useSubagentRuntimeStatus: vi.fn(),
}));

// Mock the reporter so telemetry emission does not trip jsdom/Tauri invoke paths.
vi.mock("../logging/safeReporter", () => ({
  reportFrontendEventSafely: vi.fn(),
}));

import { useSubagentRuntimeStatus } from "../api/useBusytokData";
import { reportFrontendEventSafely } from "../logging/safeReporter";
import { SubagentsPage } from "./SubagentsPage";

const mockUseStatus = vi.mocked(useSubagentRuntimeStatus);

function makePressure(
  overrides: Partial<SubagentPressureGateDto> = {},
): SubagentPressureGateDto {
  return {
    level: "normal",
    memory_used_pct: 30,
    hot_sessions_total: 1,
    hot_sessions_limit: 3,
    worker_sampled_at_ms: null,
    ...overrides,
  };
}

function makeSubagent(
  overrides: Partial<SubagentRuntimeSubagentDto> = {},
): SubagentRuntimeSubagentDto {
  return {
    name: "my-agent",
    status: "warm",
    task_count: 0,
    last_task_at_ms: null,
    last_task_status: null,
    ...overrides,
  };
}

function makeTask(
  overrides: Partial<SubagentRuntimeTaskDto> = {},
): SubagentRuntimeTaskDto {
  return {
    task_id: "t1",
    subagent_name: "my-agent",
    status: "completed",
    created_at_ms: 1000,
    error: null,
    ...overrides,
  };
}

function makeWorker(
  overrides: Partial<SubagentWorkerDto> = {},
): SubagentWorkerDto {
  return {
    provider_id: null,
    state: "running",
    pid: 12345,
    uptime_seconds: 60,
    hot_sessions: 2,
    ...overrides,
  };
}

function makeInner(
  overrides: Partial<SubagentRuntimeStatusDto> = {},
): SubagentRuntimeStatusDto {
  return {
    pressure_gate: makePressure(),
    subagents: [],
    tasks_recent: [],
    workers: [],
    ...overrides,
  };
}

function makeEnvelope(
  overrides: Partial<ReadEnvelopeDto<SubagentRuntimeStatusDto>> = {},
): ReadEnvelopeDto<SubagentRuntimeStatusDto> {
  return {
    data: makeInner(),
    generated_at_ms: 1000,
    generation_id: null,
    readiness: "ready_exact",
    is_exact: true,
    is_stale: false,
    watermark_ms: null,
    progress: null,
    degraded_reason: null,
    ...overrides,
  };
}

// Partial mock shape cast to the full hook return type. Tests only drive
// `data`, `isLoading`, `isError`, `isFetching`, so we cast via `never` to
// satisfy TypeScript without enumerating the full UseQueryResult surface.
type StatusQueryResult = ReturnType<typeof useSubagentRuntimeStatus>;

function mockStatusQuery(
  envelope: ReadEnvelopeDto<SubagentRuntimeStatusDto> | undefined,
  extras: Partial<StatusQueryResult> = {},
): StatusQueryResult {
  return {
    data: envelope,
    isLoading: false,
    isError: false,
    isFetching: false,
    ...extras,
  } as never;
}

function renderPage() {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <SubagentsPage />
    </QueryClientProvider>,
  );
}

beforeEach(() => {
  vi.clearAllMocks();
  mockUseStatus.mockReturnValue(mockStatusQuery(makeEnvelope()));
});

afterEach(() => cleanup());

describe("SubagentsPage", () => {
  it("emits page_viewed telemetry on mount", () => {
    renderPage();
    expect(reportFrontendEventSafely).toHaveBeenCalledWith(
      expect.objectContaining({ event_code: "subagent.page_viewed" }),
    );
  });

  it("renders pressure summary section with level, memory, hot sessions", () => {
    renderPage();
    expect(
      screen.getByRole("heading", { name: /pressure summary/i }),
    ).toBeTruthy();
    expect(screen.getByText("normal")).toBeTruthy();
    expect(screen.getByText("30%")).toBeTruthy();
    expect(screen.getByText("1 / 3")).toBeTruthy(); // hot_sessions_total / limit
  });

  it("renders sample freshness as em-dash when worker_sampled_at_ms is null", () => {
    renderPage();
    expect(screen.getByText("—")).toBeTruthy();
  });

  it("renders sample freshness text when worker_sampled_at_ms is set", () => {
    const sampledAt = Date.now() - 5_000;
    mockUseStatus.mockReturnValue(
      mockStatusQuery(
        makeEnvelope({
          data: makeInner({
            pressure_gate: makePressure({ worker_sampled_at_ms: sampledAt }),
          }),
        }),
      ),
    );
    renderPage();
    expect(screen.getByText(/sampled/i)).toBeTruthy();
  });

  it("renders pressure warning tone when throttled", () => {
    mockUseStatus.mockReturnValue(
      mockStatusQuery(
        makeEnvelope({
          data: makeInner({
            pressure_gate: makePressure({
              level: "throttled",
              memory_used_pct: 85,
              hot_sessions_total: 3,
            }),
          }),
        }),
      ),
    );
    renderPage();
    expect(screen.getByText("throttled")).toBeTruthy();
  });

  it("renders pressure warning tone for evicting and restarting levels", () => {
    for (const level of ["evicting", "restarting"] as const) {
      mockUseStatus.mockReturnValue(
        mockStatusQuery(
          makeEnvelope({
            data: makeInner({
              pressure_gate: makePressure({ level, memory_used_pct: 90 }),
            }),
          }),
        ),
      );
      const { unmount } = renderPage();
      // The level value renders; tone is applied via class — assert presence.
      expect(screen.getByText(level)).toBeTruthy();
      expect(
        document.querySelector(".settings-value--warning"),
      ).not.toBeNull();
      unmount();
      cleanup();
    }
  });

  it("renders empty state when no subagents", () => {
    renderPage();
    expect(screen.getByText(/no subagents/i)).toBeTruthy();
  });

  it("renders subagent rows with name, status, and task count", () => {
    mockUseStatus.mockReturnValue(
      mockStatusQuery(
        makeEnvelope({
          data: makeInner({
            subagents: [
              makeSubagent({
                name: "agent-a",
                status: "warm",
                task_count: 5,
                last_task_at_ms: 1000,
                last_task_status: "completed",
              }),
            ],
          }),
        }),
      ),
    );
    renderPage();
    expect(screen.getByText("agent-a")).toBeTruthy();
    expect(screen.getByText("warm")).toBeTruthy();
    expect(screen.getByText("5 tasks")).toBeTruthy();
  });

  it("renders empty state when no tasks", () => {
    renderPage();
    expect(screen.getByText(/no tasks/i)).toBeTruthy();
  });

  it("renders task history rows with id, subagent name, and status", () => {
    mockUseStatus.mockReturnValue(
      mockStatusQuery(
        makeEnvelope({
          data: makeInner({
            tasks_recent: [
              makeTask({
                task_id: "t1",
                subagent_name: "my-agent",
                status: "completed",
              }),
            ],
          }),
        }),
      ),
    );
    renderPage();
    expect(screen.getByText("t1")).toBeTruthy();
    expect(screen.getByText("my-agent")).toBeTruthy();
    expect(screen.getByText("completed")).toBeTruthy();
  });

  it("renders task error text for failed tasks", () => {
    mockUseStatus.mockReturnValue(
      mockStatusQuery(
        makeEnvelope({
          data: makeInner({
            tasks_recent: [
              makeTask({ status: "failed", error: "timeout exceeded" }),
            ],
          }),
        }),
      ),
    );
    renderPage();
    expect(screen.getByText(/timeout exceeded/i)).toBeTruthy();
  });

  it("renders empty workers state when no sidecar configured", () => {
    renderPage();
    expect(screen.getByText(/no sidecar configured/i)).toBeTruthy();
  });

  it("renders running worker with pid and uptime", () => {
    mockUseStatus.mockReturnValue(
      mockStatusQuery(
        makeEnvelope({
          data: makeInner({
            workers: [makeWorker({ state: "running", pid: 12345, uptime_seconds: 60 })],
          }),
        }),
      ),
    );
    renderPage();
    expect(screen.getByText("running")).toBeTruthy();
    expect(screen.getByText(/12345/)).toBeTruthy();
  });

  it("renders stopped worker when supervisor exists but child not running", () => {
    mockUseStatus.mockReturnValue(
      mockStatusQuery(
        makeEnvelope({
          data: makeInner({
            workers: [
              makeWorker({
                state: "stopped",
                pid: null,
                uptime_seconds: null,
                hot_sessions: 0,
              }),
            ],
          }),
        }),
      ),
    );
    renderPage();
    expect(screen.getByText("stopped")).toBeTruthy();
    // Empty workers state must NOT show (worker row present instead).
    expect(screen.queryByText(/no sidecar configured/i)).toBeNull();
  });

  it("shows loading state", () => {
    mockUseStatus.mockReturnValue(
      mockStatusQuery(undefined, { isLoading: true }),
    );
    renderPage();
    expect(screen.getByText("Loading subagents")).toBeTruthy();
  });

  it("shows error state", () => {
    mockUseStatus.mockReturnValue(
      mockStatusQuery(undefined, { isError: true }),
    );
    renderPage();
    expect(screen.getByText("Couldn't load subagents")).toBeTruthy();
  });

  it("shows degraded banner when envelope is_stale", () => {
    mockUseStatus.mockReturnValue(
      mockStatusQuery(
        makeEnvelope({
          is_stale: true,
          degraded_reason: "Read plane is operating in degraded mode",
        }),
      ),
    );
    renderPage();
    expect(screen.getByText(/degraded/i)).toBeTruthy();
  });

  it("does NOT render any action buttons (read-only)", () => {
    mockUseStatus.mockReturnValue(
      mockStatusQuery(
        makeEnvelope({
          data: makeInner({
            subagents: [makeSubagent()],
          }),
        }),
      ),
    );
    renderPage();
    expect(screen.queryByRole("button", { name: /hibernate/i })).toBeNull();
    expect(screen.queryByRole("button", { name: /delete/i })).toBeNull();
    expect(screen.queryByRole("button", { name: /retry/i })).toBeNull();
  });
});
