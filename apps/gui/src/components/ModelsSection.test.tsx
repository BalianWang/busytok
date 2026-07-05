import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, fireEvent, cleanup, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type {
  ProviderDto,
  ProviderListResponseDto,
  ModelCatalogEntryDto,
  ModelListResponseDto,
} from "@busytok/protocol-types";

vi.mock("../api/useBusytokData", () => ({
  useProviders: vi.fn(),
  useModels: vi.fn(),
  useModelMutations: vi.fn(),
}));
vi.mock("../logging/safeReporter", () => ({
  reportFrontendEventSafely: vi.fn(),
}));

import {
  useProviders,
  useModels,
  useModelMutations,
} from "../api/useBusytokData";
import { reportFrontendEventSafely } from "../logging/safeReporter";
import { ModelsSection } from "./ModelsSection";

const mockUseProviders = vi.mocked(useProviders);
const mockUseModels = vi.mocked(useModels);
const mockUseModelMutations = vi.mocked(useModelMutations);

function makeProvider(overrides: Partial<ProviderDto> = {}): ProviderDto {
  return {
    id: "deepseek",
    name: "DeepSeek",
    provider_kind: "openai_compatible",
    base_url: "https://api.deepseek.com/v1",
    enabled: true,
    has_api_key: true,
    created_at_ms: 0,
    updated_at_ms: 0,
    ...overrides,
  };
}

function makeModel(overrides: Partial<ModelCatalogEntryDto> = {}): ModelCatalogEntryDto {
  return {
    provider_id: "deepseek",
    provider_name: "DeepSeek",
    provider_kind: "openai_compatible",
    provider_enabled: true,
    model_db_id: "m-0001",
    model_id: "deepseek-chat",
    model_enabled: true,
    tags: ["chat"],
    display_name: null,
    reasoning: false,
    context_window: null,
    max_tokens: null,
    ...overrides,
  };
}

function makeListResponse(
  models: ModelCatalogEntryDto[] = [],
): ModelListResponseDto {
  return { models };
}

function makeProviderListResponse(
  providers: ProviderDto[] = [],
): ProviderListResponseDto {
  return { providers };
}

type ProvidersQueryResult = ReturnType<typeof useProviders>;
type ModelsQueryResult = ReturnType<typeof useModels>;
type ModelMutationsResult = ReturnType<typeof useModelMutations>;

function mockProvidersQuery(
  data: ProviderListResponseDto,
  extras: Partial<ProvidersQueryResult> = {},
): ProvidersQueryResult {
  return {
    data,
    isLoading: false,
    isError: false,
    isFetching: false,
    ...extras,
  } as never;
}

function mockModelsQuery(
  data: ModelListResponseDto | undefined,
  extras: Partial<ModelsQueryResult> = {},
): ModelsQueryResult {
  return {
    data,
    isLoading: false,
    isError: false,
    isFetching: false,
    ...extras,
  } as never;
}

interface MutationOverrides {
  createMutate?: ReturnType<typeof vi.fn>;
  updateMutate?: ReturnType<typeof vi.fn>;
  deleteMutate?: ReturnType<typeof vi.fn>;
  tagsUpdateMutate?: ReturnType<typeof vi.fn>;
  createPending?: boolean;
  updatePending?: boolean;
  deletePending?: boolean;
  tagsPending?: boolean;
}

function mockMutations(overrides: MutationOverrides = {}): ModelMutationsResult {
  return {
    createModel: {
      mutate: overrides.createMutate ?? vi.fn(),
      isPending: overrides.createPending ?? false,
    },
    updateModel: {
      mutate: overrides.updateMutate ?? vi.fn(),
      isPending: overrides.updatePending ?? false,
    },
    deleteModel: {
      mutate: overrides.deleteMutate ?? vi.fn(),
      isPending: overrides.deletePending ?? false,
    },
    tagsUpdate: {
      mutate: overrides.tagsUpdateMutate ?? vi.fn(),
      isPending: overrides.tagsPending ?? false,
    },
  } as never;
}

function renderSection() {
  const qc = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  return render(
    <QueryClientProvider client={qc}>
      <ModelsSection />
    </QueryClientProvider>,
  );
}

beforeEach(() => {
  vi.clearAllMocks();
  mockUseProviders.mockReturnValue(
    mockProvidersQuery(makeProviderListResponse([makeProvider()])),
  );
  mockUseModels.mockReturnValue(mockModelsQuery(makeListResponse([])));
  mockUseModelMutations.mockReturnValue(mockMutations());
});

afterEach(() => cleanup());

describe("ModelsSection", () => {
  it("renders the section heading and Add Model form", () => {
    renderSection();
    expect(screen.getByText("Models")).toBeTruthy();
    expect(screen.getByLabelText(/provider for new model/i)).toBeTruthy();
    expect(screen.getByLabelText(/^model id$/i)).toBeTruthy();
    expect(screen.getByLabelText(/tags for new model/i)).toBeTruthy();
    expect(screen.getByRole("button", { name: /add model/i })).toBeTruthy();
  });

  it("renders the filter controls", () => {
    renderSection();
    expect(screen.getByLabelText(/filter by provider/i)).toBeTruthy();
    expect(screen.getByLabelText(/filter by tag/i)).toBeTruthy();
    expect(screen.getByLabelText(/show disabled models/i)).toBeTruthy();
  });

  it("renders table rows for each model (model_id, provider_name, enabled, tags)", () => {
    mockUseModels.mockReturnValue(
      mockModelsQuery(
        makeListResponse([
          makeModel({
            model_db_id: "m-1",
            model_id: "deepseek-chat",
            provider_name: "DeepSeek",
            provider_id: "deepseek",
            model_enabled: true,
            tags: ["chat", "reasoning"],
          }),
          makeModel({
            model_db_id: "m-2",
            model_id: "deepseek-reasoner",
            provider_name: "DeepSeek",
            provider_id: "deepseek",
            model_enabled: false,
            tags: [],
          }),
        ]),
      ),
    );
    renderSection();
    // model_id labels appear in Toggle aria-label + Tags aria-label
    expect(screen.getByText("deepseek-chat")).toBeTruthy();
    expect(screen.getByText("deepseek-reasoner")).toBeTruthy();
    // Provider row renders "DeepSeek (deepseek)" for each model row. The
    // same text also appears in the filter dropdown and the create-form
    // dropdown, so we assert >= 2 (one per model row) rather than exactly 2.
    expect(screen.getAllByText("DeepSeek (deepseek)").length).toBeGreaterThanOrEqual(2);
    // Tags row: comma-joined for first model, "—" for empty second model
    expect(screen.getByText("chat, reasoning")).toBeTruthy();
    expect(screen.getAllByText("—").length).toBeGreaterThan(0);
  });

  it("surfaces model metadata (context_window, max_tokens, reasoning badge) in each row", () => {
    mockUseModels.mockReturnValue(
      mockModelsQuery(
        makeListResponse([
          makeModel({
            model_db_id: "m-1",
            model_id: "deepseek-chat",
            context_window: 128000,
            max_tokens: 16384,
            reasoning: true,
          }),
        ]),
      ),
    );
    renderSection();
    // Context window + max tokens are formatted with thousands separators.
    expect(screen.getByText(/Context:.*128,000.*tokens/)).toBeTruthy();
    expect(screen.getByText(/Max output:.*16,384.*tokens/)).toBeTruthy();
    // Reasoning badge appears in the metadata row. The create form's
    // checkbox label also says "Reasoning", so there are 2 matches:
    // the badge span + the create-form label.
    expect(screen.getAllByText(/^Reasoning$/)).toHaveLength(2);
  });

  it("renders em-dash fallback for null metadata and omits reasoning badge when false", () => {
    mockUseModels.mockReturnValue(
      mockModelsQuery(
        makeListResponse([
          makeModel({
            model_db_id: "m-1",
            model_id: "deepseek-chat",
            context_window: null,
            max_tokens: null,
            reasoning: false,
          }),
        ]),
      ),
    );
    renderSection();
    // Null metadata renders "—" fallback in both Context and Max output slots.
    expect(screen.getByText(/Context:.*—.*tokens/)).toBeTruthy();
    expect(screen.getByText(/Max output:.*—.*tokens/)).toBeTruthy();
    // No "Reasoning" badge in the metadata row when reasoning=false.
    // The create form's checkbox label still says "Reasoning" (1 match).
    expect(screen.getAllByText(/^Reasoning$/)).toHaveLength(1);
  });

  it("renders empty state when no models match filter", () => {
    mockUseModels.mockReturnValue(mockModelsQuery(makeListResponse([])));
    renderSection();
    expect(screen.getByText(/no models match the current filter/i)).toBeTruthy();
  });

  it("renders loading state when models query is loading", () => {
    mockUseModels.mockReturnValue({
      data: undefined,
      isLoading: true,
      isError: false,
      isFetching: false,
    } as never);
    renderSection();
    expect(screen.getByText(/loading models/i)).toBeTruthy();
  });

  it("renders error state when models query fails", () => {
    mockUseModels.mockReturnValue({
      data: undefined,
      isLoading: false,
      isError: true,
      isFetching: false,
    } as never);
    renderSection();
    expect(screen.getByText("Models unavailable")).toBeTruthy();
  });

  it("changes to filterProvider trigger a new models query with the new providerId", () => {
    renderSection();
    const select = screen.getByLabelText(/filter by provider/i) as HTMLSelectElement;
    fireEvent.change(select, { target: { value: "deepseek" } });
    // useModels was called with providerId="deepseek" on the re-render.
    expect(mockUseModels).toHaveBeenCalledWith(
      expect.objectContaining({ providerId: "deepseek" }),
    );
  });

  it("changes to filterTag trigger a new models query with parsed tags", () => {
    renderSection();
    const input = screen.getByLabelText(/filter by tag/i) as HTMLInputElement;
    fireEvent.change(input, { target: { value: "chat, reasoning" } });
    expect(mockUseModels).toHaveBeenCalledWith(
      expect.objectContaining({ tags: ["chat", "reasoning"] }),
    );
  });

  it("toggling showAll triggers a new models query with includeDisabled=true", () => {
    renderSection();
    const toggle = screen.getByLabelText(/show disabled models/i);
    fireEvent.click(toggle);
    expect(mockUseModels).toHaveBeenCalledWith(
      expect.objectContaining({ includeDisabled: true }),
    );
  });

  it("shows an error when Add Model is clicked without selecting a provider", () => {
    renderSection();
    fireEvent.change(screen.getByLabelText(/^model id$/i), {
      target: { value: "deepseek-chat" },
    });
    fireEvent.click(screen.getByRole("button", { name: /add model/i }));
    expect(screen.getByText(/select a provider before adding a model/i)).toBeTruthy();
  });

  it("shows an error when Add Model is clicked with an empty model id", () => {
    renderSection();
    fireEvent.change(screen.getByLabelText(/provider for new model/i), {
      target: { value: "deepseek" },
    });
    fireEvent.click(screen.getByRole("button", { name: /add model/i }));
    expect(screen.getByText(/model id cannot be empty/i)).toBeTruthy();
  });

  it("renders context_window and max_tokens as required inputs in the create form", () => {
    renderSection();
    expect(screen.getByPlaceholderText(/context window/i)).toBeTruthy();
    expect(screen.getByPlaceholderText(/max tokens/i)).toBeTruthy();
    // Optional metadata inputs are also rendered.
    expect(screen.getByPlaceholderText(/display name/i)).toBeTruthy();
    expect(screen.getByLabelText(/reasoning/i)).toBeTruthy();
  });

  it("blocks model create when context_window is missing", () => {
    const createMutate = vi.fn();
    mockUseModelMutations.mockReturnValue(mockMutations({ createMutate }));
    renderSection();
    fireEvent.change(screen.getByLabelText(/provider for new model/i), {
      target: { value: "deepseek" },
    });
    fireEvent.change(screen.getByLabelText(/^model id$/i), {
      target: { value: "deepseek-chat" },
    });
    // Fill max_tokens but leave context_window empty.
    fireEvent.change(screen.getByLabelText(/max tokens/i), {
      target: { value: "8192" },
    });
    fireEvent.click(screen.getByRole("button", { name: /add model/i }));
    expect(screen.getByText(/context window is required/i)).toBeTruthy();
    expect(createMutate).not.toHaveBeenCalled();
  });

  it("blocks model create when max_tokens is missing", () => {
    const createMutate = vi.fn();
    mockUseModelMutations.mockReturnValue(mockMutations({ createMutate }));
    renderSection();
    fireEvent.change(screen.getByLabelText(/provider for new model/i), {
      target: { value: "deepseek" },
    });
    fireEvent.change(screen.getByLabelText(/^model id$/i), {
      target: { value: "deepseek-chat" },
    });
    // Fill context_window but leave max_tokens empty.
    fireEvent.change(screen.getByLabelText(/context window/i), {
      target: { value: "64000" },
    });
    fireEvent.click(screen.getByRole("button", { name: /add model/i }));
    expect(screen.getByText(/max tokens is required/i)).toBeTruthy();
    expect(createMutate).not.toHaveBeenCalled();
  });

  it("submits display_name + reasoning from the create form when provided", () => {
    const createMutate = vi.fn(
      (
        _payload: unknown,
        opts?: { onSuccess?: (entry: ModelCatalogEntryDto) => void },
      ) => {
        opts?.onSuccess?.(makeModel({ model_db_id: "m-new" }));
      },
    );
    mockUseModelMutations.mockReturnValue(mockMutations({ createMutate }));
    renderSection();
    fireEvent.change(screen.getByLabelText(/provider for new model/i), {
      target: { value: "deepseek" },
    });
    fireEvent.change(screen.getByLabelText(/^model id$/i), {
      target: { value: "deepseek-chat" },
    });
    fireEvent.change(screen.getByLabelText(/context window/i), {
      target: { value: "64000" },
    });
    fireEvent.change(screen.getByLabelText(/max tokens/i), {
      target: { value: "8192" },
    });
    fireEvent.change(screen.getByLabelText(/display name/i), {
      target: { value: "DeepSeek Chat" },
    });
    fireEvent.click(screen.getByLabelText(/reasoning/i));
    fireEvent.click(screen.getByRole("button", { name: /add model/i }));
    expect(createMutate).toHaveBeenCalledWith(
      expect.objectContaining({
        display_name: "DeepSeek Chat",
        reasoning: true,
      }),
      expect.anything(),
    );
  });

  it("submits the create form and calls createModel.mutate with the parsed payload", () => {
    const createMutate = vi.fn(
      (
        _payload: unknown,
        opts?: {
          onSuccess?: (entry: ModelCatalogEntryDto) => void;
          onError?: (err: Error) => void;
        },
      ) => {
        opts?.onSuccess?.(makeModel({ model_db_id: "m-new", model_id: "deepseek-chat" }));
      },
    );
    mockUseModelMutations.mockReturnValue(mockMutations({ createMutate }));
    renderSection();
    fireEvent.change(screen.getByLabelText(/provider for new model/i), {
      target: { value: "deepseek" },
    });
    fireEvent.change(screen.getByLabelText(/^model id$/i), {
      target: { value: "deepseek-chat" },
    });
    fireEvent.change(screen.getByLabelText(/context window/i), {
      target: { value: "64000" },
    });
    fireEvent.change(screen.getByLabelText(/max tokens/i), {
      target: { value: "8192" },
    });
    fireEvent.change(screen.getByLabelText(/tags for new model/i), {
      target: { value: "chat, reasoning" },
    });
    fireEvent.click(screen.getByRole("button", { name: /add model/i }));
    expect(createMutate).toHaveBeenCalledTimes(1);
    expect(createMutate).toHaveBeenCalledWith(
      expect.objectContaining({
        provider_id: "deepseek",
        model_id: "deepseek-chat",
        enabled: true,
        tags: ["chat", "reasoning"],
        context_window: 64000,
        max_tokens: 8192,
        display_name: null,
        reasoning: false,
      }),
      expect.anything(),
    );
    // On success the form resets.
    expect((screen.getByLabelText(/^model id$/i) as HTMLInputElement).value).toBe("");
    // Telemetry emitted.
    expect(vi.mocked(reportFrontendEventSafely)).toHaveBeenCalledWith(
      expect.objectContaining({ event_code: "model.added" }),
    );
  });

  it("shows an error when createModel fails", () => {
    const createMutate = vi.fn(
      (_payload: unknown, opts?: { onError?: (err: Error) => void }) => {
        opts?.onError?.(new Error("model create failed"));
      },
    );
    mockUseModelMutations.mockReturnValue(mockMutations({ createMutate }));
    renderSection();
    fireEvent.change(screen.getByLabelText(/provider for new model/i), {
      target: { value: "deepseek" },
    });
    fireEvent.change(screen.getByLabelText(/^model id$/i), {
      target: { value: "deepseek-chat" },
    });
    fireEvent.change(screen.getByLabelText(/context window/i), {
      target: { value: "64000" },
    });
    fireEvent.change(screen.getByLabelText(/max tokens/i), {
      target: { value: "8192" },
    });
    fireEvent.click(screen.getByRole("button", { name: /add model/i }));
    expect(screen.getByText("model create failed")).toBeTruthy();
  });

  it("toggling a model's enabled state calls updateModel.mutate with model_db_id and inverted enabled", () => {
    const updateMutate = vi.fn();
    mockUseModelMutations.mockReturnValue(mockMutations({ updateMutate }));
    mockUseModels.mockReturnValue(
      mockModelsQuery(
        makeListResponse([
          makeModel({ model_db_id: "m-1", model_id: "deepseek-chat", model_enabled: true }),
        ]),
      ),
    );
    renderSection();
    const toggle = screen.getByLabelText(/toggle deepseek-chat/i);
    fireEvent.click(toggle);
    expect(updateMutate).toHaveBeenCalledWith(
      {
        id: "m-1",
        enabled: false,
        display_name: null,
        reasoning: null,
        context_window: null,
        max_tokens: null,
      },
      expect.anything(),
    );
  });

  it("shows an error when toggle fails", () => {
    const updateMutate = vi.fn(
      (_payload: unknown, opts?: { onError?: (err: Error) => void }) => {
        opts?.onError?.(new Error("toggle failed"));
      },
    );
    mockUseModelMutations.mockReturnValue(mockMutations({ updateMutate }));
    mockUseModels.mockReturnValue(
      mockModelsQuery(
        makeListResponse([
          makeModel({ model_db_id: "m-1", model_id: "deepseek-chat", model_enabled: true }),
        ]),
      ),
    );
    renderSection();
    fireEvent.click(screen.getByLabelText(/toggle deepseek-chat/i));
    expect(screen.getByText("toggle failed")).toBeTruthy();
  });

  it("calls deleteModel.mutate with model_db_id when Delete is clicked (after confirm)", () => {
    const deleteMutate = vi.fn(
      (_id: string, opts?: { onSuccess?: () => void; onError?: (err: Error) => void }) => {
        opts?.onSuccess?.();
      },
    );
    const confirmSpy = vi.spyOn(globalThis, "confirm").mockReturnValue(true);
    mockUseModelMutations.mockReturnValue(mockMutations({ deleteMutate }));
    mockUseModels.mockReturnValue(
      mockModelsQuery(
        makeListResponse([
          makeModel({
            model_db_id: "m-1",
            model_id: "deepseek-chat",
            provider_name: "DeepSeek",
          }),
        ]),
      ),
    );
    renderSection();
    fireEvent.click(screen.getByRole("button", { name: /delete/i }));
    expect(confirmSpy).toHaveBeenCalled();
    expect(deleteMutate).toHaveBeenCalledWith("m-1", expect.anything());
    expect(vi.mocked(reportFrontendEventSafely)).toHaveBeenCalledWith(
      expect.objectContaining({ event_code: "model.deleted" }),
    );
    confirmSpy.mockRestore();
  });

  it("does not call deleteModel.mutate when confirm is cancelled", () => {
    const deleteMutate = vi.fn();
    const confirmSpy = vi.spyOn(globalThis, "confirm").mockReturnValue(false);
    mockUseModelMutations.mockReturnValue(mockMutations({ deleteMutate }));
    mockUseModels.mockReturnValue(
      mockModelsQuery(
        makeListResponse([makeModel({ model_db_id: "m-1", model_id: "deepseek-chat" })]),
      ),
    );
    renderSection();
    fireEvent.click(screen.getByRole("button", { name: /delete/i }));
    expect(confirmSpy).toHaveBeenCalled();
    expect(deleteMutate).not.toHaveBeenCalled();
    confirmSpy.mockRestore();
  });

  it("shows an error when delete fails", () => {
    const deleteMutate = vi.fn(
      (_id: string, opts?: { onError?: (err: Error) => void }) => {
        opts?.onError?.(new Error("delete failed"));
      },
    );
    const confirmSpy = vi.spyOn(globalThis, "confirm").mockReturnValue(true);
    mockUseModelMutations.mockReturnValue(mockMutations({ deleteMutate }));
    mockUseModels.mockReturnValue(
      mockModelsQuery(
        makeListResponse([makeModel({ model_db_id: "m-1", model_id: "deepseek-chat" })]),
      ),
    );
    renderSection();
    fireEvent.click(screen.getByRole("button", { name: /delete/i }));
    expect(screen.getByText("delete failed")).toBeTruthy();
    confirmSpy.mockRestore();
  });

  it("Edit Tags button reveals a tag editor pre-filled with the current tags", () => {
    mockUseModels.mockReturnValue(
      mockModelsQuery(
        makeListResponse([
          makeModel({ model_db_id: "m-1", model_id: "deepseek-chat", tags: ["chat", "fast"] }),
        ]),
      ),
    );
    renderSection();
    fireEvent.click(screen.getByRole("button", { name: /edit tags/i }));
    const input = screen.getByLabelText(/tags for deepseek-chat/i) as HTMLInputElement;
    expect(input.value).toBe("chat, fast");
  });

  it("Save Tags button calls tagsUpdate.mutate with parsed tags and exits edit mode", () => {
    const tagsUpdateMutate = vi.fn(
      (
        _payload: unknown,
        opts?: { onSuccess?: () => void; onError?: (e: Error) => void },
      ) => {
        opts?.onSuccess?.();
      },
    );
    mockUseModelMutations.mockReturnValue(mockMutations({ tagsUpdateMutate }));
    mockUseModels.mockReturnValue(
      mockModelsQuery(
        makeListResponse([
          makeModel({ model_db_id: "m-1", model_id: "deepseek-chat", tags: ["chat"] }),
        ]),
      ),
    );
    renderSection();
    fireEvent.click(screen.getByRole("button", { name: /edit tags/i }));
    const input = screen.getByLabelText(/tags for deepseek-chat/i) as HTMLInputElement;
    fireEvent.change(input, { target: { value: "chat, reasoning, " } });
    fireEvent.click(screen.getByRole("button", { name: /save tags/i }));
    expect(tagsUpdateMutate).toHaveBeenCalledWith(
      { modelId: "m-1", tags: ["chat", "reasoning"] },
      expect.anything(),
    );
    // Edit mode exited: tag input is gone, view-mode label is shown.
    expect(screen.queryByLabelText(/tags for deepseek-chat/i)).toBeNull();
    expect(screen.getByText("chat")).toBeTruthy();
    // Edit Tags button re-rendered (so the user can edit again).
    expect(screen.getByRole("button", { name: /edit tags/i })).toBeTruthy();
  });

  it("Cancel button exits tag edit mode without calling tagsUpdate.mutate", () => {
    const tagsUpdateMutate = vi.fn();
    mockUseModelMutations.mockReturnValue(mockMutations({ tagsUpdateMutate }));
    mockUseModels.mockReturnValue(
      mockModelsQuery(
        makeListResponse([
          makeModel({ model_db_id: "m-1", model_id: "deepseek-chat", tags: ["chat"] }),
        ]),
      ),
    );
    renderSection();
    fireEvent.click(screen.getByRole("button", { name: /edit tags/i }));
    fireEvent.click(screen.getByRole("button", { name: /cancel/i }));
    expect(tagsUpdateMutate).not.toHaveBeenCalled();
    // Tag edit input is gone; static tags label is shown again.
    expect(screen.queryByLabelText(/tags for deepseek-chat/i)).toBeNull();
    expect(screen.getByText("chat")).toBeTruthy();
  });

  it("shows an error when tagsUpdate fails", async () => {
    const tagsUpdateMutate = vi.fn(
      (_payload: unknown, opts?: { onError?: (err: Error) => void }) => {
        opts?.onError?.(new Error("tags save failed"));
      },
    );
    mockUseModelMutations.mockReturnValue(mockMutations({ tagsUpdateMutate }));
    mockUseModels.mockReturnValue(
      mockModelsQuery(
        makeListResponse([
          makeModel({ model_db_id: "m-1", model_id: "deepseek-chat", tags: ["chat"] }),
        ]),
      ),
    );
    renderSection();
    fireEvent.click(screen.getByRole("button", { name: /edit tags/i }));
    fireEvent.change(screen.getByLabelText(/tags for deepseek-chat/i), {
      target: { value: "chat, reasoning" },
    });
    fireEvent.click(screen.getByRole("button", { name: /save tags/i }));
    await waitFor(() => {
      expect(screen.getByText("tags save failed")).toBeTruthy();
    });
  });

  it("disables mutation buttons while a mutation is pending", () => {
    mockUseModelMutations.mockReturnValue(
      mockMutations({
        createPending: true,
        updatePending: true,
        deletePending: true,
        tagsPending: true,
      }),
    );
    mockUseModels.mockReturnValue(
      mockModelsQuery(
        makeListResponse([makeModel({ model_db_id: "m-1", model_id: "deepseek-chat" })]),
      ),
    );
    renderSection();
    // Create button shows "Saving..." (only one matches before opening the tag editor).
    expect(screen.getByRole("button", { name: /^saving\.\.\.$/i })).toBeTruthy();
    // Delete button shows "Deleting...".
    expect(screen.getByRole("button", { name: /deleting/i })).toBeTruthy();
    // The Save Tags button only renders after clicking Edit Tags. After
    // clicking, it also shows "Saving..." (tagsPending=true), so there
    // are now TWO "Saving..." buttons (create + save tags).
    fireEvent.click(screen.getByRole("button", { name: /edit tags/i }));
    expect(screen.getAllByRole("button", { name: /^saving\.\.\.$/i }).length).toBe(2);
    // Toggle disabled
    const toggle = screen.getByLabelText(/toggle deepseek-chat/i) as HTMLInputElement;
    expect(toggle.disabled).toBe(true);
  });
});
