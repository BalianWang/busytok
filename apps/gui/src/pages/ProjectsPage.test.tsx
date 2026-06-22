import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type {
  BreakdownListResponseDto,
  BreakdownListItemDto,
  BreakdownDetailDto,
  ProjectBreakdownListItemDto,
  ProjectBreakdownDetailDto,
  ReadEnvelopeDto
} from "@busytok/protocol-types";


function envelope<T>(data: T): ReadEnvelopeDto<T> {
  return {
    data,
    generated_at_ms: 0,
    generation_id: null,
    readiness: "ready_exact" as any,
    is_exact: true,
    is_stale: false,
    watermark_ms: null,
    progress: null,
    degraded_reason: null,
  };
}

// Mock the API hooks
const mockUseBreakdownList = vi.fn();
const mockUseBreakdownDetail = vi.fn();
vi.mock("../api/useBusytokData", () => ({
  useBreakdownList: (...args: unknown[]) => mockUseBreakdownList(...args),
  useBreakdownDetail: (...args: unknown[]) => mockUseBreakdownDetail(...args),
}));

import { ProjectsPage } from "./ProjectsPage";

const mockRefetch = vi.fn();

function makeProjectItem(
  overrides: Partial<ProjectBreakdownListItemDto> = {},
): BreakdownListItemDto {
  return {
    kind: "project",
    id: "proj-1",
    project_hash: "abc123def456",
    label: "my-app",
    subtitle: null,
    tokens: 25000,
    cost_usd: 0.35,
    cost_status: "exact",
    event_count: 42,
    last_active_at_ms: Date.now() - 3600_000,
    top_model_label: "claude-sonnet-4",
    ...overrides,
  } as BreakdownListItemDto;
}

function makeResponse(
  overrides: Partial<BreakdownListResponseDto> = {},
  items: BreakdownListItemDto[] = [makeProjectItem()],
): BreakdownListResponseDto {
  return {
    generated_at_ms: Date.now(),
    kind: "project",
    items,
    next_cursor: null,
    summary: {
      item_count: items.length,
      total_tokens: items.reduce((s, i) => s + (i as ProjectBreakdownListItemDto).tokens, 0),
      total_cost_usd: items.reduce((s, i) => s + ((i as ProjectBreakdownListItemDto).cost_usd ?? 0), 0),
      total_cost_status: "exact",
    },
    ...overrides,
  };
}

function makeProjectDetail(
  overrides: Partial<ProjectBreakdownDetailDto> = {},
): BreakdownDetailDto {
  return {
    kind: "project",
    id: "proj-1",
    label: "my-app",
    project_hash: "abc123def456",
    project_path: "/Users/me/projects/my-app",
    metrics: [],
    trend: {
      range: "month",
      bucket_granularity: "day",
      metric_options: ["tokens", "cost"],
      cost_status: "exact",
      buckets: [],
    },
    model_mix: [],
    sessions: [],
    recent_activity: [],
    technical_details: [],
    ...overrides,
  } as BreakdownDetailDto;
}

function mockSuccess(
  data: BreakdownListResponseDto = makeResponse(),
  detailData?: BreakdownDetailDto,
) {
  mockUseBreakdownList.mockReturnValue({
    data: envelope(data),
    isLoading: false,
    isError: false,
    isFetching: false,
    refetch: mockRefetch,
  });
  mockUseBreakdownDetail.mockReturnValue({
    data: envelope(detailData ?? makeProjectDetail()),
    isLoading: false,
    isError: false,
    refetch: vi.fn(),
  });
}

function mockLoading() {
  mockUseBreakdownList.mockReturnValue({
    data: undefined,
    isLoading: true,
    isError: false,
    isFetching: true,
    refetch: mockRefetch,
  });
}

function mockError() {
  mockUseBreakdownList.mockReturnValue({
    data: undefined,
    isLoading: false,
    isError: true,
    isFetching: false,
    refetch: mockRefetch,
  });
}

function mockEmpty() {
  mockUseBreakdownList.mockReturnValue({
    data: envelope(makeResponse({}, [])),
    isLoading: false,
    isError: false,
    isFetching: false,
    refetch: mockRefetch,
  });
}

beforeEach(() => {
  document.body.innerHTML = "";
  mockUseBreakdownList.mockReset();
  mockUseBreakdownDetail.mockReset();
});

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

describe("ProjectsPage", () => {
  it("shows loading state initially", () => {
    mockLoading();
    render(<ProjectsPage />);
    expect(screen.getByText(/loading projects data/i)).toBeDefined();
  });

  it("shows error state with retry button", () => {
    mockError();
    render(<ProjectsPage />);
    expect(screen.getByText(/projects unavailable/i)).toBeDefined();
    expect(screen.getByText(/retry/i)).toBeDefined();
  });

  it("calls refetch when retry is clicked", async () => {
    const user = userEvent.setup();
    mockError();
    render(<ProjectsPage />);
    await user.click(screen.getByText(/retry/i));
    expect(mockRefetch).toHaveBeenCalled();
  });

  it("shows empty state when no items", () => {
    mockEmpty();
    render(<ProjectsPage />);
    expect(screen.getByText(/no data/i)).toBeDefined();
  });

  it("does not render a page hero heading", () => {
    mockSuccess();
    render(<ProjectsPage />);
    expect(screen.queryByRole("heading", { name: "Projects" })).toBeNull();
  });

  it("renders range segmented control with Month default", () => {
    mockSuccess();
    render(<ProjectsPage />);
    expect(screen.getByRole("group", { name: "Range" })).toBeDefined();
    const monthBtn = screen.getByRole("button", { name: "Month" });
    expect(monthBtn).toBeDefined();
    expect(monthBtn.getAttribute("aria-pressed")).toBe("true");
  });

  it("renders ledger table with project data", () => {
    mockSuccess(
      makeResponse({}, [
        makeProjectItem({
          id: "p1",
          label: "my-app",
          tokens: 25000,
          cost_usd: 0.35,
          event_count: 42,
          top_model_label: "claude-sonnet-4",
        }),
      ]),
    );
    render(<ProjectsPage />);
    expect(screen.getByText("my-app")).toBeDefined();
    expect(screen.getByText("25,000")).toBeDefined();
    expect(screen.getByText("$0.35")).toBeDefined();
    expect(screen.getByText("42")).toBeDefined();
    expect(screen.getByText("claude-sonnet-4")).toBeDefined();
  });

  it("shows -- when top model is null", () => {
    mockSuccess(
      makeResponse({}, [
        makeProjectItem({ top_model_label: null }),
      ]),
    );
    render(<ProjectsPage />);
    const dashes = screen.getAllByText("--");
    expect(dashes.length).toBeGreaterThanOrEqual(1);
  });

  it("renders pagination summary", () => {
    mockSuccess(
      makeResponse({}, [
        makeProjectItem({ id: "p1" }),
        makeProjectItem({ id: "p2" }),
      ]),
    );
    render(<ProjectsPage />);
    expect(screen.getByText(/1–2/)).toBeDefined();
  });

  it("renders next page button when nextCursor is set", () => {
    mockSuccess(makeResponse({ next_cursor: "cursor-2" }));
    render(<ProjectsPage />);
    expect(screen.getByText("Next")).toBeDefined();
  });

  it("disables next page button when nextCursor is null", () => {
    mockSuccess();
    render(<ProjectsPage />);
    const btn = screen.getByText("Next");
    expect(btn.closest("button")).toHaveProperty("disabled", true);
  });

  it("opens detail drawer when a row is clicked", async () => {
    const item = makeProjectItem({ id: "proj-click", label: "clicked-project" });
    const detail = makeProjectDetail({
      id: "proj-click",
      project_hash: "hash123",
      project_path: "/custom/path",
    });
    const user = userEvent.setup();
    mockSuccess(makeResponse({}, [item]), detail);
    render(<ProjectsPage />);

    await user.click(screen.getByText("clicked-project"));

    await waitFor(() => {
      expect(screen.getByText("Project Detail")).toBeDefined();
    });
    expect(screen.getByText("/custom/path")).toBeDefined();
    expect(screen.getByText("hash123")).toBeDefined();
  });

  it("renders cost as N/A when cost_status is unavailable", () => {
    const item = makeProjectItem({
      cost_usd: null,
      cost_status: "unavailable",
    });
    mockSuccess(makeResponse({}, [item]));
    render(<ProjectsPage />);
    expect(screen.getAllByText("N/A").length).toBeGreaterThanOrEqual(1);
  });

  it("renders partial cost same as exact", () => {
    const item = makeProjectItem({
      cost_usd: 1.5,
      cost_status: "partial",
    });
    mockSuccess(makeResponse({}, [item]));
    render(<ProjectsPage />);
    expect(screen.getByText("$1.50")).toBeDefined();
  });

  it("shows detail drawer loading state", async () => {
    const item = makeProjectItem({ id: "proj-load", label: "loading-project" });
    const user = userEvent.setup();

    // Make detail hook return loading (not yet ready)
    mockUseBreakdownDetail.mockReturnValue({
      data: undefined,
      isLoading: true,
      isError: false,
      refetch: vi.fn(),
    });
    mockUseBreakdownList.mockReturnValue({
      data: envelope(makeResponse({}, [item])),
      isLoading: false,
      isError: false,
      isFetching: false,
      refetch: mockRefetch,
    });

    render(<ProjectsPage />);
    await user.click(screen.getByText("loading-project"));

    await waitFor(() => {
      expect(screen.getByText("Loading detail...")).toBeDefined();
    });
  });

  it("shows detail drawer empty state when detail returns no data", async () => {
    const item = makeProjectItem({ id: "proj-empty", label: "empty-project" });
    const user = userEvent.setup();

    mockUseBreakdownDetail.mockReturnValue({
      data: undefined,
      isLoading: false,
      isError: false,
      refetch: vi.fn(),
    });
    mockUseBreakdownList.mockReturnValue({
      data: envelope(makeResponse({}, [item])),
      isLoading: false,
      isError: false,
      isFetching: false,
      refetch: mockRefetch,
    });

    render(<ProjectsPage />);
    await user.click(screen.getByText("empty-project"));

    await waitFor(() => {
      expect(screen.getByText("No detail available.")).toBeDefined();
    });
  });

  it("shows detail drawer with technical details", async () => {
    const item = makeProjectItem({ id: "proj-tech", label: "tech-project" });
    const detail = makeProjectDetail({
      id: "proj-tech",
      technical_details: [
        { label: "Source ID", value: "src-1" },
        { label: "Provider", value: "Anthropic" },
      ],
    });
    const user = userEvent.setup();
    mockSuccess(makeResponse({}, [item]), detail);
    render(<ProjectsPage />);

    await user.click(screen.getByText("tech-project"));

    await waitFor(() => {
      expect(screen.getByText("Technical Details")).toBeDefined();
    });
    expect(screen.getByText("Anthropic")).toBeDefined();
    expect(screen.getByText("src-1")).toBeDefined();
  });

  it("displays model mix in detail drawer", async () => {
    const item = makeProjectItem({ id: "proj-models", label: "models-project" });
    const detail = makeProjectDetail({
      id: "proj-models",
      model_mix: [
        { id: "m1", label: "claude-sonnet-4", tokens: 15000, cost_usd: 0.20, cost_status: "exact", event_count: 10 },
        { id: "m2", label: "gpt-4o", tokens: 10000, cost_usd: 0.15, cost_status: "exact", event_count: 8 },
      ],
    });
    const user = userEvent.setup();
    mockSuccess(makeResponse({}, [item]), detail);
    render(<ProjectsPage />);

    await user.click(screen.getByText("models-project"));

    await waitFor(() => {
      expect(screen.getByText("Models (2)")).toBeDefined();
    });
    // gpt-4o is unique to the drawer (not in the table row)
    expect(screen.getByText("gpt-4o")).toBeDefined();
    // claude-sonnet-4 appears in both table and drawer, so use getAllByText
    const sonnetMatches = screen.getAllByText("claude-sonnet-4");
    expect(sonnetMatches.length).toBeGreaterThanOrEqual(2);
  });

  it("displays recent activity in detail drawer", async () => {
    const item = makeProjectItem({ id: "proj-act", label: "activity-project" });
    const detail = makeProjectDetail({
      id: "proj-act",
      recent_activity: [
        { id: "a1", happened_at_ms: Date.now() - 60000, client_id: "c1", client_label: "Claude Code", source_id: null, source_label: null, source_root_path: null, project_label: null, project_hash: null, model_id: null, model_label: "claude-sonnet-4", tokens: 1500, cache_hit_rate: null, cost_usd: 0.015, cost_status: "exact", status: "ok", detail_available: false },
      ],
    });
    const user = userEvent.setup();
    mockSuccess(makeResponse({}, [item]), detail);
    render(<ProjectsPage />);

    await user.click(screen.getByText("activity-project"));

    await waitFor(() => {
      expect(screen.getByText("Recent Activity (1)")).toBeDefined();
    });
    expect(screen.getByText("ok")).toBeDefined();
  });
});
