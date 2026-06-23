import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type {
  ActivityListResponseDto,
  ActivityDetailDto,
  ActivityListItemDto,
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
const mockUseActivityList = vi.fn();
const mockUseActivityDetail = vi.fn();
vi.mock("../api/useBusytokData", () => ({
  useActivityList: (...args: unknown[]) => mockUseActivityList(...args),
  useActivityDetail: (...args: unknown[]) => mockUseActivityDetail(...args),
}));

import { ActivityPage } from "./ActivityPage";

const mockRefetch = vi.fn();

function makeItem(
  overrides: Partial<ActivityListItemDto> = {},
): ActivityListItemDto {
  return {
    id: "evt-1",
    happened_at_ms: 1716000000000,
    client_id: "client-1",
    client_label: "Claude Code",
    source_id: "src-1",
    source_label: "/projects/myapp",
    source_root_path: "/projects",
    project_label: "MyApp",
    project_hash: "abc123",
    model_id: "model-1",
    model_label: "claude-sonnet-4",
    tokens: 1500,
    cache_hit_rate: 0.0,
    cost_usd: 0.015,
    cost_status: "exact",
    status: "ok",
    detail_available: true,
    ...overrides,
  };
}

function makeResponse(
  overrides: Partial<ActivityListResponseDto> = {},
  items: ActivityListItemDto[] = [makeItem()],
): ActivityListResponseDto {
  return {
    generated_at_ms: 1716000000000,
    items,
    next_cursor: null,
    summary: {
      item_count: items.length,
      total_tokens: items.reduce((s, i) => s + i.tokens, 0),
      total_cost_usd: items.reduce((s, i) => s + (i.cost_usd ?? 0), 0),
      cost_status: "exact",
    },
    ...overrides,
  };
}

function makeDetail(
  overrides: Partial<ActivityDetailDto> = {},
): ActivityDetailDto {
  return {
    id: "evt-1",
    title: "Usage Event",
    subtitle: null,
    happened_at_ms: 1716000000000,
    client_id: "client-1",
    client_label: "Claude Code",
    source_id: "src-1",
    source_label: "/projects/myapp",
    source_root_path: "/projects",
    project_label: "MyApp",
    project_hash: "abc123",
    session_id: "sess-1",
    model_id: "model-1",
    model_label: "claude-sonnet-4",
    status: "ok",
    tokens: 1500,
    token_breakdown: {
      prompt_input_total_tokens: 800,
      prompt_input_non_cached_tokens: 700,
      cache_read_tokens: 100,
      cache_write_tokens: 0,
      cache_hit_rate: 0.125,
      input_tokens: 800,
      output_tokens: 700,
      cached_input_tokens: 100,
      reasoning_tokens: 50,
      total_tokens: 1500,
    },
    cost_usd: 0.015,
    cost_status: "exact",
    technical_details: {
      source_id: "src-1",
      provider: "Anthropic",
      raw_model: "claude-sonnet-4-20250101",
      notes: ["Cached context used"],
    },
    ...overrides,
  };
}

function mockSuccess(
  data: ActivityListResponseDto = makeResponse(),
  detailData?: ActivityDetailDto,
) {
  mockUseActivityList.mockReturnValue({
    data: envelope(data),
    isLoading: false,
    isError: false,
    isFetching: false,
    refetch: mockRefetch,
  });
  mockUseActivityDetail.mockReturnValue({
    data: envelope(detailData ?? makeDetail()),
    isLoading: false,
    isError: false,
    refetch: vi.fn(),
  });
}

function mockLoading() {
  mockUseActivityList.mockReturnValue({
    data: undefined,
    isLoading: true,
    isError: false,
    isFetching: true,
    refetch: mockRefetch,
  });
}

function mockError() {
  mockUseActivityList.mockReturnValue({
    data: undefined,
    isLoading: false,
    isError: true,
    isFetching: false,
    refetch: mockRefetch,
  });
}

function mockEmpty() {
  mockUseActivityList.mockReturnValue({
    data: envelope(makeResponse({}, [])),
    isLoading: false,
    isError: false,
    isFetching: false,
    refetch: mockRefetch,
  });
}

beforeEach(() => {
  document.body.innerHTML = "";
  mockUseActivityList.mockReset();
  mockUseActivityDetail.mockReset();
});

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

describe("ActivityPage", () => {
  it("shows loading state initially", () => {
    mockLoading();
    render(<ActivityPage />);
    expect(screen.getByText(/loading activity/i)).toBeDefined();
  });

  it("shows error state with retry button", () => {
    mockError();
    render(<ActivityPage />);
    expect(screen.getByText(/activity unavailable/i)).toBeDefined();
    expect(screen.getByText(/retry/i)).toBeDefined();
  });

  it("calls refetch when retry is clicked", async () => {
    const user = userEvent.setup();
    mockError();
    render(<ActivityPage />);
    await user.click(screen.getByText(/retry/i));
    expect(mockRefetch).toHaveBeenCalled();
  });

  it("shows empty state when no items", () => {
    mockEmpty();
    render(<ActivityPage />);
    expect(screen.getByRole("heading", { name: /no activity/i })).toBeDefined();
  });

  it("does not render a page hero heading", () => {
    mockSuccess();
    render(<ActivityPage />);
    expect(screen.queryByRole("heading", { name: "Activity" })).toBeNull();
  });

  it("shows range hint that activity is retained for 24 hours", () => {
    mockSuccess();
    render(<ActivityPage />);
    expect(screen.getByText(/today's activity/i)).toBeDefined();
  });


  it("renders ledger table with data columns", () => {
    mockSuccess(
      makeResponse(
        {},
        [
          makeItem({
            id: "evt-1",
            client_label: "Claude Code",
            model_label: "claude-sonnet-4",
            tokens: 1500,
            cost_usd: 0.015,
            status: "ok",
          }),
        ],
      ),
    );
    render(<ActivityPage />);
    expect(screen.getByText("Claude Code")).toBeDefined();
    expect(screen.getByText("claude-sonnet-4")).toBeDefined();
    expect(screen.getByText("1,500")).toBeDefined();
    expect(screen.getByText("ok")).toBeDefined();
    // Project column
    expect(screen.getByText("MyApp")).toBeDefined();
  });

  it("renders pagination summary", () => {
    mockSuccess(makeResponse({}, [makeItem(), makeItem({ id: "evt-2" })]));
    render(<ActivityPage />);
    expect(screen.getByText(/Showing/)).toBeDefined();
    expect(screen.getByText(/1–2/)).toBeDefined();
  });

  it("opens detail drawer when a selectable row is clicked", async () => {
    const item = makeItem({ id: "evt-click", detail_available: true });
    const detail = makeDetail({ id: "evt-click", client_label: "Claude Code" });
    const user = userEvent.setup();
    mockSuccess(makeResponse({}, [item]), detail);
    render(<ActivityPage />);

    // Click on rendered cell content to trigger row selection
    const clientLabel = screen.getByText("Claude Code");
    await user.click(clientLabel);

    // DetailDrawer should open with title "Activity Detail"
    await waitFor(() => {
      expect(screen.getByText("Activity Detail")).toBeDefined();
    });
    // Claude Code now appears both in table and drawer
    const matches = screen.getAllByText("Claude Code");
    expect(matches.length).toBeGreaterThanOrEqual(2);
  });

  it("does not open drawer when detail_available is false", async () => {
    const item = makeItem({
      id: "evt-disabled",
      detail_available: false,
      client_label: "Codex",
    });
    const user = userEvent.setup();
    mockSuccess(makeResponse({}, [item]));
    render(<ActivityPage />);

    await user.click(screen.getByText("Codex"));
    expect(screen.queryByText("Activity Detail")).toBeNull();
  });

  it("renders cost as N/A when cost_status is unavailable", () => {
    const item = makeItem({
      cost_usd: null,
      cost_status: "unavailable",
    });
    mockSuccess(makeResponse({}, [item]));
    render(<ActivityPage />);
    expect(screen.getAllByText("N/A").length).toBeGreaterThanOrEqual(1);
  });

  it("renders partial cost same as exact", () => {
    const item = makeItem({
      cost_usd: 1.5,
      cost_status: "partial",
    });
    mockSuccess(makeResponse({}, [item]));
    render(<ActivityPage />);
    expect(screen.getByText("$1.50")).toBeDefined();
  });

  it("renders status pills with status classes", () => {
    mockSuccess(
      makeResponse(
        {},
        [
          makeItem({ status: "ok" }),
          makeItem({ id: "evt-warn", status: "warning" }),
          makeItem({ id: "evt-err", status: "error" }),
        ],
      ),
    );
    render(<ActivityPage />);

    const pills = document.querySelectorAll(".status-pill");
    expect(pills.length).toBe(3);
    expect(pills[0].className).toContain("status-pill--ok");
    expect(pills[1].className).toContain("status-pill--warning");
    expect(pills[2].className).toContain("status-pill--error");
  });

  it("shows -- for null project label", () => {
    const item = makeItem({ project_label: null });
    mockSuccess(makeResponse({}, [item]));
    render(<ActivityPage />);
    const dashes = screen.getAllByText("--");
    expect(dashes.length).toBeGreaterThanOrEqual(1);
  });

  it("renders next page button when nextCursor is set", () => {
    mockSuccess(makeResponse({ next_cursor: "cursor-2" }));
    render(<ActivityPage />);
    expect(screen.getByText("Next")).toBeDefined();
  });

  it("disables next page button when nextCursor is null", () => {
    mockSuccess();
    render(<ActivityPage />);
    const btn = screen.getByText("Next");
    expect(btn).toBeDefined();
    expect(btn.closest("button")).toHaveProperty("disabled", true);
  });

  it("renders detail drawer content with token breakdown", async () => {
    const item = makeItem({ id: "evt-detail" });
    const detail = makeDetail({
      id: "evt-detail",
      token_breakdown: {
        prompt_input_total_tokens: 800,
        prompt_input_non_cached_tokens: 700,
        cache_read_tokens: 100,
        cache_write_tokens: 0,
        cache_hit_rate: 0.125,
        input_tokens: 800,
        output_tokens: 700,
        cached_input_tokens: 100,
        reasoning_tokens: 50,
        total_tokens: 1500,
      },
    });
    const user = userEvent.setup();
    mockSuccess(makeResponse({}, [item]), detail);
    render(<ActivityPage />);

    await user.click(screen.getByText("Claude Code"));

    await waitFor(() => {
      expect(screen.getByText("Activity Detail")).toBeDefined();
    });
    // Token breakdown should be visible (appears in both table head and drawer)
    const tokenHeadings = screen.getAllByText("Tokens");
    expect(tokenHeadings.length).toBe(2);
    // Technical details should be visible
    expect(screen.getByText("Technical Details")).toBeDefined();
    expect(screen.getByText("Anthropic")).toBeDefined();
    expect(screen.getByText("Cached context used")).toBeDefined();
  });

  it("displays cost in detail drawer", async () => {
    const item = makeItem({ id: "evt-cost" });
    const detail = makeDetail({ id: "evt-cost", cost_usd: 0.05, cost_status: "exact" });
    const user = userEvent.setup();
    mockSuccess(makeResponse({}, [item]), detail);
    render(<ActivityPage />);

    await user.click(screen.getByText("Claude Code"));

    await waitFor(() => {
      expect(screen.getByText("Activity Detail")).toBeDefined();
    });
    expect(screen.getByText("$0.050")).toBeDefined();
  });

  it("renders Cache column header in the ledger table", () => {
    const item = makeItem({ cache_hit_rate: 0.3 });
    mockSuccess(makeResponse({}, [item]));
    render(<ActivityPage />);
    // Table header "Cache" should be present
    expect(screen.getByText("Cache")).toBeDefined();
  });

  it("renders cache hit rate percentage in list row", () => {
    const item = makeItem({ cache_hit_rate: 0.3 });
    mockSuccess(makeResponse({}, [item]));
    render(<ActivityPage />);
    // 0.3 → "30.00%"
    expect(screen.getByText("30.00%")).toBeDefined();
  });

  it("renders -- for null cache_hit_rate in list row", () => {
    const item = makeItem({ cache_hit_rate: null });
    mockSuccess(makeResponse({}, [item]));
    render(<ActivityPage />);
    // null should display as "--"
    expect(screen.getByText("--")).toBeDefined();
  });

  it("renders Cache Hit Rate in detail drawer from the DTO field", async () => {
    const item = makeItem({ id: "evt-cache" });
    const detail = makeDetail({
      id: "evt-cache",
      token_breakdown: {
        prompt_input_total_tokens: 1000,
        prompt_input_non_cached_tokens: 700,
        cache_read_tokens: 300,
        cache_write_tokens: 0,
        cache_hit_rate: 0.3,
        input_tokens: 1000,
        output_tokens: 400,
        cached_input_tokens: 300,
        reasoning_tokens: null,
        total_tokens: 1400,
      },
    });
    const user = userEvent.setup();
    mockSuccess(makeResponse({}, [item]), detail);
    render(<ActivityPage />);

    await user.click(screen.getByText("Claude Code"));

    await waitFor(() => {
      expect(screen.getByText("Activity Detail")).toBeDefined();
    });
    expect(screen.getByText("Cache Hit Rate")).toBeDefined();
    // Rate is read from token_breakdown.cache_hit_rate (0.3) → "30.00%"
    // — NOT recomputed from cached_input_tokens / input_tokens.
    expect(screen.getByText("30.00%")).toBeDefined();
  });

  it("shows the DTO cache_hit_rate in the detail drawer without recomputing", async () => {
    const item = makeItem({ id: "evt-dto" });
    const detail = makeDetail({
      id: "evt-dto",
      token_breakdown: {
        prompt_input_total_tokens: 1000,
        prompt_input_non_cached_tokens: 10,
        cache_read_tokens: 990,
        cache_write_tokens: 0,
        cache_hit_rate: 0.99,
        input_tokens: 1000,
        output_tokens: 400,
        // Deliberately inconsistent: cached_input_tokens alone would give
        // a different rate (0.1) if the client recomputed. The DTO rate wins.
        cached_input_tokens: 100,
        reasoning_tokens: null,
        total_tokens: 1400,
      },
    });
    const user = userEvent.setup();
    mockSuccess(makeResponse({}, [item]), detail);
    render(<ActivityPage />);

    await user.click(screen.getByText("Claude Code"));

    await waitFor(() => {
      expect(screen.getByText("Activity Detail")).toBeDefined();
    });
    expect(screen.getByText(/99\.00%/)).toBeDefined();
  });

  it("clears selection when Next is clicked", async () => {
    const item = makeItem({ id: "evt-sel", detail_available: true });
    const detail = makeDetail({ id: "evt-sel" });
    const user = userEvent.setup();
    mockSuccess(makeResponse({ next_cursor: "cursor-2" }, [item]), detail);
    render(<ActivityPage />);

    // Open detail drawer
    await user.click(screen.getByText("Claude Code"));
    await waitFor(() => {
      expect(screen.getByText("Activity Detail")).toBeDefined();
    });

    // Click Next — should close the drawer
    await user.click(screen.getByText("Next"));
    await waitFor(() => {
      expect(screen.queryByText("Activity Detail")).toBeNull();
    });
  });

  it("clears selection when page size is changed", async () => {
    const item = makeItem({ id: "evt-ps", detail_available: true });
    const detail = makeDetail({ id: "evt-ps" });
    const user = userEvent.setup();
    mockSuccess(makeResponse({}, [item]), detail);
    render(<ActivityPage />);

    await user.click(screen.getByText("Claude Code"));
    await waitFor(() => {
      expect(screen.getByText("Activity Detail")).toBeDefined();
    });

    // Click rows 25 — should close the drawer
    await user.click(screen.getByText("25"));
    await waitFor(() => {
      expect(screen.queryByText("Activity Detail")).toBeNull();
    });
  });
});
