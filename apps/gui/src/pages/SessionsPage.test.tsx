import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type {
  BreakdownListResponseDto,
  BreakdownListItemDto,
  BreakdownDetailDto,
  SessionBreakdownListItemDto,
  SessionBreakdownDetailDto,
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

import { SessionsPage } from "./SessionsPage";

const mockRefetch = vi.fn();

function makeSessionItem(
  overrides: Partial<SessionBreakdownListItemDto> = {},
): BreakdownListItemDto {
  return {
    kind: "session",
    id: "sess-1",
    label: "Session #1",
    subtitle: null,
    tokens: 15000,
    cost_usd: 0.25,
    cost_status: "exact",
    event_count: 30,
    last_active_at_ms: Date.now() - 3600_000,
    client_label: "Claude Code",
    project_label: "my-app",
    project_hash: "abc123",
    ...overrides,
  } as BreakdownListItemDto;
}

function makeResponse(
  overrides: Partial<BreakdownListResponseDto> = {},
  items: BreakdownListItemDto[] = [makeSessionItem()],
): BreakdownListResponseDto {
  return {
    generated_at_ms: Date.now(),
    kind: "session",
    items,
    next_cursor: null,
    summary: {
      item_count: items.length,
      total_tokens: items.reduce((s, i) => s + (i as SessionBreakdownListItemDto).tokens, 0),
      total_cost_usd: items.reduce((s, i) => s + ((i as SessionBreakdownListItemDto).cost_usd ?? 0), 0),
      total_cost_status: "exact",
    },
    ...overrides,
  };
}

function makeSessionDetail(
  overrides: Partial<SessionBreakdownDetailDto> = {},
): BreakdownDetailDto {
  return {
    kind: "session",
    id: "sess-1",
    label: "Session #1",
    client_id: "client-1",
    client_label: "Claude Code",
    project_label: "my-app",
    project_hash: "abc123",
    last_active_at_ms: Date.now() - 3600_000,
    metrics: [],
    token_breakdown: {
      prompt_input_total_tokens: 10000,
      prompt_input_non_cached_tokens: 8000,
      cache_read_tokens: 2000,
      cache_write_tokens: 0,
      cache_hit_rate: 0.2,
      total_tokens: 15000,
      input_tokens: 10000,
      output_tokens: 5000,
      cached_input_tokens: 2000,
      reasoning_tokens: 500,
    },
    timeline: [],
    models_used: [],
    source_context: [],
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
    data: envelope(detailData ?? makeSessionDetail()),
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

describe("SessionsPage", () => {
  it("shows loading state initially", () => {
    mockLoading();
    render(<SessionsPage />);
    expect(screen.getByText(/loading sessions data/i)).toBeDefined();
  });

  it("shows error state with retry button", () => {
    mockError();
    render(<SessionsPage />);
    expect(screen.getByText(/sessions unavailable/i)).toBeDefined();
    expect(screen.getByText(/retry/i)).toBeDefined();
  });

  it("calls refetch when retry is clicked", async () => {
    const user = userEvent.setup();
    mockError();
    render(<SessionsPage />);
    await user.click(screen.getByText(/retry/i));
    expect(mockRefetch).toHaveBeenCalled();
  });

  it("shows empty state when no items", () => {
    mockEmpty();
    render(<SessionsPage />);
    expect(screen.getByText(/no data/i)).toBeDefined();
  });

  it("does not render a page hero heading", () => {
    mockSuccess();
    render(<SessionsPage />);
    expect(screen.queryByRole("heading", { name: "Sessions" })).toBeNull();
  });

  it("renders range segmented control with Month default", () => {
    mockSuccess();
    render(<SessionsPage />);
    expect(screen.getByRole("group", { name: "Range" })).toBeDefined();
    const monthBtn = screen.getByRole("button", { name: "Month" });
    expect(monthBtn).toBeDefined();
    expect(monthBtn.getAttribute("aria-pressed")).toBe("true");
  });

  it("renders ledger table with session data", () => {
    mockSuccess(
      makeResponse({}, [
        makeSessionItem({
          id: "s1",
          label: "Session #1",
          client_label: "Claude Code",
          project_label: "my-app",
          tokens: 15000,
          event_count: 30,
        }),
      ]),
    );
    render(<SessionsPage />);
    expect(screen.getByText("Session #1")).toBeDefined();
    expect(screen.getByText("Claude Code")).toBeDefined();
    expect(screen.getByText("my-app")).toBeDefined();
    expect(screen.getByText("15,000")).toBeDefined();
    expect(screen.getByText("30")).toBeDefined();
  });

  it("shows -- for null project_label in list", () => {
    mockSuccess(
      makeResponse({}, [
        makeSessionItem({ project_label: null }),
      ]),
    );
    render(<SessionsPage />);
    const dashes = screen.getAllByText("--");
    expect(dashes.length).toBeGreaterThanOrEqual(1);
  });

  it("renders pagination summary", () => {
    mockSuccess(
      makeResponse({}, [
        makeSessionItem({ id: "s1" }),
        makeSessionItem({ id: "s2" }),
      ]),
    );
    render(<SessionsPage />);
    expect(screen.getByText(/1–2/)).toBeDefined();
  });

  it("renders next page button when nextCursor is set", () => {
    mockSuccess(makeResponse({ next_cursor: "cursor-2" }));
    render(<SessionsPage />);
    expect(screen.getByText("Next")).toBeDefined();
  });

  it("opens detail drawer when a row is clicked", async () => {
    const item = makeSessionItem({ id: "sess-click", label: "Clicked Session" });
    const detail = makeSessionDetail({
      id: "sess-click",
      client_label: "Claude Code",
      project_label: "my-app",
      project_hash: "abc123",
    });
    const user = userEvent.setup();
    mockSuccess(makeResponse({}, [item]), detail);
    render(<SessionsPage />);

    await user.click(screen.getByText("Clicked Session"));

    await waitFor(() => {
      expect(screen.getByText("Session Detail")).toBeDefined();
    });
    // Project hash should be in the drawer (not in the main table)
    expect(screen.getByText("abc123")).toBeDefined();
  });

  it("shows detail drawer loading state", async () => {
    const item = makeSessionItem({ id: "sess-load", label: "Loading Session" });
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

    render(<SessionsPage />);
    await user.click(screen.getByText("Loading Session"));

    await waitFor(() => {
      expect(screen.getByText("Loading detail...")).toBeDefined();
    });
  });

  it("shows detail drawer empty state when detail returns no data", async () => {
    const item = makeSessionItem({ id: "sess-empty", label: "Empty Session" });
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

    render(<SessionsPage />);
    await user.click(screen.getByText("Empty Session"));

    await waitFor(() => {
      expect(screen.getByText("No detail available.")).toBeDefined();
    });
  });

  it("displays token breakdown in detail drawer", async () => {
    const item = makeSessionItem({ id: "sess-tokens", label: "Token Session" });
    const detail = makeSessionDetail({
      id: "sess-tokens",
      token_breakdown: {
        prompt_input_total_tokens: 15000,
        prompt_input_non_cached_tokens: 12000,
        cache_read_tokens: 3000,
        cache_write_tokens: 0,
        cache_hit_rate: 0.2,
        total_tokens: 25000,
        input_tokens: 15000,
        output_tokens: 10000,
        cached_input_tokens: 3000,
        reasoning_tokens: 2000,
      },
    });
    const user = userEvent.setup();
    mockSuccess(makeResponse({}, [item]), detail);
    render(<SessionsPage />);

    await user.click(screen.getByText("Token Session"));

    await waitFor(() => {
      expect(screen.getByText("Token Breakdown")).toBeDefined();
    });
    // Tokens appear in both table and drawer, so use getAllByText
    const totalMatches = screen.getAllByText("25,000");
    expect(totalMatches.length).toBeGreaterThanOrEqual(1);
    const inputMatches = screen.getAllByText("15,000");
    expect(inputMatches.length).toBeGreaterThanOrEqual(1);
    const outputMatches = screen.getAllByText("10,000");
    expect(outputMatches.length).toBeGreaterThanOrEqual(1);
  });

  it("displays timeline in detail drawer", async () => {
    const item = makeSessionItem({ id: "sess-time", label: "Timeline Session" });
    const detail = makeSessionDetail({
      id: "sess-time",
      timeline: [
        { id: "t1", happened_at_ms: Date.now() - 60000, label: "Initial request", tokens: 5000, cost_usd: 0.05, cost_status: "exact", status: "ok" },
        { id: "t2", happened_at_ms: Date.now() - 30000, label: "Follow-up", tokens: 3000, cost_usd: 0.03, cost_status: "exact", status: "ok" },
      ],
    });
    const user = userEvent.setup();
    mockSuccess(makeResponse({}, [item]), detail);
    render(<SessionsPage />);

    await user.click(screen.getByText("Timeline Session"));

    await waitFor(() => {
      expect(screen.getByText("Timeline (2)")).toBeDefined();
    });
    expect(screen.getByText("Initial request")).toBeDefined();
    expect(screen.getByText("Follow-up")).toBeDefined();
  });

  it("displays models used in detail drawer", async () => {
    const item = makeSessionItem({ id: "sess-models", label: "Models Session" });
    const detail = makeSessionDetail({
      id: "sess-models",
      models_used: [
        { id: "m1", label: "claude-sonnet-4", tokens: 10000, cost_usd: 0.15, cost_status: "exact", event_count: 5 },
        { id: "m2", label: "gpt-4o", tokens: 5000, cost_usd: 0.10, cost_status: "exact", event_count: 3 },
      ],
    });
    const user = userEvent.setup();
    mockSuccess(makeResponse({}, [item]), detail);
    render(<SessionsPage />);

    await user.click(screen.getByText("Models Session"));

    await waitFor(() => {
      expect(screen.getByText("Models Used (2)")).toBeDefined();
    });
    expect(screen.getByText("claude-sonnet-4")).toBeDefined();
    expect(screen.getByText("gpt-4o")).toBeDefined();
  });

  it("displays source context in detail drawer", async () => {
    const item = makeSessionItem({ id: "sess-source", label: "Source Session" });
    const detail = makeSessionDetail({
      id: "sess-source",
      source_context: [
        { source_id: "src-1", client_label: "Claude Code", root_path: "/projects/my-app" },
      ],
    });
    const user = userEvent.setup();
    mockSuccess(makeResponse({}, [item]), detail);
    render(<SessionsPage />);

    await user.click(screen.getByText("Source Session"));

    await waitFor(() => {
      expect(screen.getByText("Source Context")).toBeDefined();
    });
    // Claude Code appears in both table and drawer; /projects/my-app should be unique
    expect(screen.getByText("/projects/my-app")).toBeDefined();
  });

  it("displays technical details in drawer", async () => {
    const item = makeSessionItem({ id: "sess-tech", label: "Tech Session" });
    const detail = makeSessionDetail({
      id: "sess-tech",
      technical_details: [
        { label: "Client ID", value: "client-1" },
        { label: "Session Duration", value: "2h 15m" },
      ],
    });
    const user = userEvent.setup();
    mockSuccess(makeResponse({}, [item]), detail);
    render(<SessionsPage />);

    await user.click(screen.getByText("Tech Session"));

    await waitFor(() => {
      expect(screen.getByText("Technical Details")).toBeDefined();
    });
    expect(screen.getByText("client-1")).toBeDefined();
    expect(screen.getByText("2h 15m")).toBeDefined();
  });

  it("clears detail drawer when Next is clicked", async () => {
    const item = makeSessionItem({ id: "sess-sel", label: "Selected Session" });
    const detail = makeSessionDetail({ id: "sess-sel" });
    const user = userEvent.setup();
    mockSuccess(makeResponse({ next_cursor: "cursor-2" }, [item]), detail);
    render(<SessionsPage />);

    await user.click(screen.getByText("Selected Session"));
    await waitFor(() => {
      expect(screen.getByText("Session Detail")).toBeDefined();
    });

    await user.click(screen.getByText("Next"));
    await waitFor(() => {
      expect(screen.queryByText("Session Detail")).toBeNull();
    });
  });
});
