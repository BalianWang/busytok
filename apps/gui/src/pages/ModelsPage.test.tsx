import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type {
  BreakdownListResponseDto,
  BreakdownListItemDto,
  BreakdownDetailDto,
  ModelBreakdownListItemDto,
  ModelBreakdownDetailDto,
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

import { ModelsPage } from "./ModelsPage";

const mockRefetch = vi.fn();

function makeModelItem(
  overrides: Partial<ModelBreakdownListItemDto> = {},
): BreakdownListItemDto {
  return {
    kind: "model",
    id: "model-1",
    label: "claude-sonnet-4",
    subtitle: null,
    tokens: 50000,
    cost_usd: 0.75,
    cost_status: "exact",
    event_count: 85,
    last_active_at_ms: Date.now() - 7200_000,
    client_labels: ["Claude Code", "Codex"],
    top_project_label: "my-app",
    ...overrides,
  } as BreakdownListItemDto;
}

function makeResponse(
  overrides: Partial<BreakdownListResponseDto> = {},
  items: BreakdownListItemDto[] = [makeModelItem()],
): BreakdownListResponseDto {
  return {
    generated_at_ms: Date.now(),
    kind: "model",
    items,
    next_cursor: null,
    summary: {
      item_count: items.length,
      total_tokens: items.reduce((s, i) => s + (i as ModelBreakdownListItemDto).tokens, 0),
      total_cost_usd: items.reduce((s, i) => s + ((i as ModelBreakdownListItemDto).cost_usd ?? 0), 0),
      total_cost_status: "exact",
    },
    ...overrides,
  };
}

function makeModelDetail(
  overrides: Partial<ModelBreakdownDetailDto> = {},
): BreakdownDetailDto {
  return {
    kind: "model",
    id: "model-1",
    label: "claude-sonnet-4",
    metrics: [],
    trend: {
      range: "month",
      bucket_granularity: "day",
      metric_options: ["tokens", "cost"],
      cost_status: "exact",
      buckets: [],
    },
    token_breakdown: {
      prompt_input_total_tokens: 30000,
      prompt_input_non_cached_tokens: 25000,
      cache_read_tokens: 5000,
      cache_write_tokens: 0,
      cache_hit_rate: 0.16666,
      total_tokens: 50000,
      input_tokens: 30000,
      output_tokens: 20000,
      cached_input_tokens: 5000,
      reasoning_tokens: 1000,
    },
    client_mix: [],
    project_mix: [],
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
    data: envelope(detailData ?? makeModelDetail()),
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

describe("ModelsPage", () => {
  it("shows loading state initially", () => {
    mockLoading();
    render(<ModelsPage />);
    expect(screen.getByText(/loading models data/i)).toBeDefined();
  });

  it("shows error state with retry button", () => {
    mockError();
    render(<ModelsPage />);
    expect(screen.getByText(/models unavailable/i)).toBeDefined();
    expect(screen.getByText(/retry/i)).toBeDefined();
  });

  it("calls refetch when retry is clicked", async () => {
    const user = userEvent.setup();
    mockError();
    render(<ModelsPage />);
    await user.click(screen.getByText(/retry/i));
    expect(mockRefetch).toHaveBeenCalled();
  });

  it("shows empty state when no items", () => {
    mockEmpty();
    render(<ModelsPage />);
    expect(screen.getByText(/no data/i)).toBeDefined();
  });

  it("does not render a page hero heading", () => {
    mockSuccess();
    render(<ModelsPage />);
    expect(screen.queryByRole("heading", { name: "Models" })).toBeNull();
  });

  it("renders range segmented control with Month default", () => {
    mockSuccess();
    render(<ModelsPage />);
    expect(screen.getByRole("group", { name: "Range" })).toBeDefined();
    const monthBtn = screen.getByRole("button", { name: "Month" });
    expect(monthBtn).toBeDefined();
    expect(monthBtn.getAttribute("aria-pressed")).toBe("true");
  });

  it("renders ledger table with model data", () => {
    mockSuccess(
      makeResponse({}, [
        makeModelItem({
          id: "m1",
          label: "claude-sonnet-4",
          tokens: 50000,
          cost_usd: 0.75,
          event_count: 85,
          client_labels: ["Claude Code"],
        }),
      ]),
    );
    render(<ModelsPage />);
    expect(screen.getByText("claude-sonnet-4")).toBeDefined();
    expect(screen.getByText("50,000")).toBeDefined();
    expect(screen.getByText("$0.75")).toBeDefined();
    expect(screen.getByText("85")).toBeDefined();
    expect(screen.getByText("Claude Code")).toBeDefined();
  });

  it("shows -- for empty client_labels", () => {
    mockSuccess(
      makeResponse({}, [
        makeModelItem({ client_labels: [] }),
      ]),
    );
    render(<ModelsPage />);
    const dashes = screen.getAllByText("--");
    expect(dashes.length).toBeGreaterThanOrEqual(1);
  });

  it("renders pagination summary", () => {
    mockSuccess(
      makeResponse({}, [
        makeModelItem({ id: "m1" }),
        makeModelItem({ id: "m2" }),
      ]),
    );
    render(<ModelsPage />);
    expect(screen.getByText(/1–2/)).toBeDefined();
  });

  it("renders next page button when nextCursor is set", () => {
    mockSuccess(makeResponse({ next_cursor: "cursor-2" }));
    render(<ModelsPage />);
    expect(screen.getByText("Next")).toBeDefined();
  });

  it("opens detail drawer when a row is clicked", async () => {
    const item = makeModelItem({ id: "model-click", label: "gpt-4o" });
    const detail = makeModelDetail({
      id: "model-click",
      token_breakdown: {
        prompt_input_total_tokens: 20000,
        prompt_input_non_cached_tokens: 20000,
        cache_read_tokens: 0,
        cache_write_tokens: 0,
        cache_hit_rate: 0.0,
        total_tokens: 30000,
        input_tokens: 20000,
        output_tokens: 10000,
        cached_input_tokens: null,
        reasoning_tokens: null,
      },
    });
    const user = userEvent.setup();
    mockSuccess(makeResponse({}, [item]), detail);
    render(<ModelsPage />);

    await user.click(screen.getByText("gpt-4o"));

    await waitFor(() => {
      expect(screen.getByText("Model Detail")).toBeDefined();
    });
    expect(screen.getByText("Token Breakdown")).toBeDefined();
    expect(screen.getByText("30,000")).toBeDefined();
    // "20,000" appears for both Prompt Input (Total) and Input tokens.
    expect(screen.getAllByText("20,000").length).toBeGreaterThanOrEqual(1);
    expect(screen.getByText("10,000")).toBeDefined();
  });

  it("renders cost as N/A when cost_status is unavailable", () => {
    const item = makeModelItem({
      cost_usd: null,
      cost_status: "unavailable",
    });
    mockSuccess(makeResponse({}, [item]));
    render(<ModelsPage />);
    expect(screen.getAllByText("N/A").length).toBeGreaterThanOrEqual(1);
  });

  it("renders partial cost same as exact", () => {
    const item = makeModelItem({
      cost_usd: 2.5,
      cost_status: "partial",
    });
    mockSuccess(makeResponse({}, [item]));
    render(<ModelsPage />);
    expect(screen.getByText("$2.50")).toBeDefined();
  });

  it("shows detail drawer loading state", async () => {
    const item = makeModelItem({ id: "model-load", label: "loading-model" });
    const user = userEvent.setup();

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

    render(<ModelsPage />);
    await user.click(screen.getByText("loading-model"));

    await waitFor(() => {
      expect(screen.getByText("Loading detail...")).toBeDefined();
    });
  });

  it("shows detail drawer empty state when detail returns no data", async () => {
    const item = makeModelItem({ id: "model-empty", label: "empty-model" });
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

    render(<ModelsPage />);
    await user.click(screen.getByText("empty-model"));

    await waitFor(() => {
      expect(screen.getByText("No detail available.")).toBeDefined();
    });
  });

  it("displays client mix in detail drawer", async () => {
    const item = makeModelItem({ id: "model-clients", label: "clients-model" });
    const detail = makeModelDetail({
      id: "model-clients",
      client_mix: [
        { id: "c1", label: "Claude Code", tokens: 30000, cost_usd: 0.45, cost_status: "exact", event_count: 50 },
        { id: "c2", label: "Codex", tokens: 20000, cost_usd: 0.30, cost_status: "exact", event_count: 35 },
      ],
    });
    const user = userEvent.setup();
    mockSuccess(makeResponse({}, [item]), detail);
    render(<ModelsPage />);

    await user.click(screen.getByText("clients-model"));

    await waitFor(() => {
      expect(screen.getByText("Clients (2)")).toBeDefined();
    });
    expect(screen.getByText("Claude Code")).toBeDefined();
  });

  it("displays project mix in detail drawer", async () => {
    const item = makeModelItem({ id: "model-projects", label: "projects-model" });
    const detail = makeModelDetail({
      id: "model-projects",
      project_mix: [
        { id: "p1", project_hash: "hash1", label: "my-app", subtitle: null, tokens: 30000, cost_usd: 0.45, cost_status: "exact", event_count: 50, last_active_at_ms: null, top_model_label: null },
      ],
    });
    const user = userEvent.setup();
    mockSuccess(makeResponse({}, [item]), detail);
    render(<ModelsPage />);

    await user.click(screen.getByText("projects-model"));

    await waitFor(() => {
      expect(screen.getByText("Projects (1)")).toBeDefined();
    });
    expect(screen.getByText("my-app")).toBeDefined();
  });

  it("displays recent activity in detail drawer", async () => {
    const item = makeModelItem({ id: "model-recent", label: "recent-model" });
    const detail = makeModelDetail({
      id: "model-recent",
      recent_activity: [
        {
          id: "a1",
          happened_at_ms: Date.now() - 600_000,
          client_id: "claude-code",
          client_label: "Claude Code",
          source_id: null,
          source_label: null,
          source_root_path: null,
          project_hash: null,
          project_label: "my-app",
          model_id: "claude-sonnet-4",
          model_label: "claude-sonnet-4",
          tokens: 1234,
          cost_usd: 0.01,
          cost_status: "exact",
          cache_hit_rate: 0.99,
          status: "ok",
          detail_available: true,
        },
      ],
    });
    const user = userEvent.setup();
    mockSuccess(makeResponse({}, [item]), detail);
    render(<ModelsPage />);

    await user.click(screen.getByText("recent-model"));

    await waitFor(() => {
      expect(screen.getByText("Recent Activity (1)")).toBeDefined();
    });
    expect(screen.getByText("claude-sonnet-4")).toBeDefined();
  });

  it("displays technical details in drawer", async () => {
    const item = makeModelItem({ id: "model-tech", label: "tech-model" });
    const detail = makeModelDetail({
      id: "model-tech",
      technical_details: [
        { label: "Provider", value: "Anthropic" },
        { label: "Context Window", value: "200K" },
      ],
    });
    const user = userEvent.setup();
    mockSuccess(makeResponse({}, [item]), detail);
    render(<ModelsPage />);

    await user.click(screen.getByText("tech-model"));

    await waitFor(() => {
      expect(screen.getByText("Technical Details")).toBeDefined();
    });
    expect(screen.getByText("Anthropic")).toBeDefined();
    expect(screen.getByText("200K")).toBeDefined();
  });
});
