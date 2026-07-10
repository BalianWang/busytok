import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, render, screen, fireEvent, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type {
  ModelCatalogEntryDto,
  ModelListResponseDto,
  ProviderDto,
  ProviderListResponseDto,
} from "@busytok/protocol-types";

vi.mock("../api/useBusytokData", () => ({
  useProviders: vi.fn(),
  useProviderMutations: vi.fn(),
  useModels: vi.fn(),
  useModelMutations: vi.fn(),
}));
vi.mock("../logging/safeReporter", () => ({
  reportFrontendEventSafely: vi.fn(),
}));

import { ProvidersPage } from "./ProvidersPage";
import {
  useProviders,
  useProviderMutations,
  useModels,
  useModelMutations,
} from "../api/useBusytokData";
import { reportFrontendEventSafely } from "../logging/safeReporter";

const makeProvider = (overrides: Partial<ProviderDto> = {}): ProviderDto => ({
  id: "prov-1",
  name: "deepseek_openai",
  provider_kind: "openai_compatible" as never,
  base_url: "https://api.deepseek.com/v1",
  enabled: true,
  has_api_key: true,
  created_at_ms: 0,
  updated_at_ms: 0,
  ...overrides,
});

const makeModel = (overrides: Partial<ModelCatalogEntryDto> = {}): ModelCatalogEntryDto => ({
  provider_id: "prov-1",
  provider_name: "deepseek_openai",
  provider_kind: "openai_compatible" as never,
  provider_enabled: true,
  model_db_id: "model-db-1",
  model_id: "deepseek-chat",
  model_enabled: true,
  tags: [],
  display_name: "deepseek-chat",
  reasoning: false,
  context_window: 200000,
  max_tokens: 8192,
  ...overrides,
});

type ProviderMutationsResult = ReturnType<typeof useProviderMutations>;
type ModelMutationsResult = ReturnType<typeof useModelMutations>;

const mockMutations = (): ProviderMutationsResult => ({
  createProvider: { mutate: vi.fn(), isPending: false },
  updateProvider: { mutate: vi.fn(), isPending: false },
  deleteProvider: {
    mutate: vi.fn((_id: string, opts?: { onSuccess?: () => void }) => {
      opts?.onSuccess?.();
    }),
    mutateAsync: vi.fn().mockResolvedValue(undefined),
    isPending: false,
  },
  testConnection: {
    mutate: vi.fn(
      (
        _id: string,
        opts?: { onSuccess?: (r: unknown) => void; onError?: (e: Error) => void },
      ) => {
        opts?.onSuccess?.({ ok: true, error: null, models_detected: null });
      },
    ),
    isPending: false,
  },
} as never);

const mockModelMutations = (): ModelMutationsResult => ({
  // createModel returns ModelCatalogEntryDto (per useBusytokData.ts:470);
  // the page's handleModelCreate reads entry.provider_id + entry.model_id, so
  // the mock must resolve with a concrete entry or the handler throws.
  createModel: {
    mutate: vi.fn(),
    mutateAsync: vi.fn().mockResolvedValue({
      provider_id: "prov-1",
      provider_name: "deepseek_openai",
      provider_kind: "openai_compatible" as never,
      provider_enabled: true,
      model_db_id: "model-db-new",
      model_id: "new-model",
      model_enabled: true,
      tags: [],
      display_name: "new-model",
      reasoning: false,
      context_window: 200000,
      max_tokens: 8192,
    }),
    isPending: false,
  },
  updateModel: {
    mutate: vi.fn(),
    mutateAsync: vi.fn().mockResolvedValue(undefined),
    isPending: false,
  },
  deleteModel: {
    mutate: vi.fn((_id: string, opts?: { onSuccess?: () => void }) => {
      opts?.onSuccess?.();
    }),
    mutateAsync: vi.fn().mockResolvedValue(undefined),
    isPending: false,
  },
  tagsUpdate: {
    mutate: vi.fn(),
    mutateAsync: vi.fn().mockResolvedValue(undefined),
    isPending: false,
  },
} as never);

function renderPage(
  overrides: { providers?: ProviderDto[]; models?: ModelCatalogEntryDto[] } = {},
) {
  vi.mocked(useProviders).mockReturnValue({
    data: { providers: overrides.providers ?? [] } as ProviderListResponseDto,
    isLoading: false,
    isError: false,
    isFetching: false,
  } as never);
  vi.mocked(useModels).mockReturnValue({
    data: { models: overrides.models ?? [] } as ModelListResponseDto,
    isLoading: false,
    isError: false,
    isFetching: false,
  } as never);
  // useProviderMutations / useModelMutations are set in beforeEach; tests
  // that need a custom mutation mock must call mockReturnValue BEFORE
  // renderPage (renderPage does not overwrite them).
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <ProvidersPage />
    </QueryClientProvider>,
  );
}

beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(useProviderMutations).mockReturnValue(mockMutations());
  vi.mocked(useModelMutations).mockReturnValue(mockModelMutations());
});

afterEach(() => cleanup());

describe("ProvidersPage (rewritten)", () => {
  it("renders empty state when no providers", () => {
    renderPage();
    expect(screen.getByText(/新建 Provider/i)).toBeTruthy();
  });

  it("shows creation form when + 新建 button clicked", () => {
    renderPage();
    fireEvent.click(screen.getByRole("button", { name: /\+ 新建/i }));
    expect(screen.getByPlaceholderText(/base url/i)).toBeTruthy();
  });

  it("closes creation form when 取消 clicked (exercises onClose callback)", () => {
    renderPage();
    fireEvent.click(screen.getByRole("button", { name: /\+ 新建/i }));
    expect(screen.getByPlaceholderText(/base url/i)).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: /^取消$/i }));
    expect(screen.queryByPlaceholderText(/base url/i)).toBeNull();
  });

  it("enters and exits provider edit mode (exercises onEdit + onCancelEdit)", () => {
    renderPage({ providers: [makeProvider()] });
    // Click the provider's 编辑 button (first one — in the card header).
    const editButtons = screen.getAllByRole("button", { name: /编辑/i });
    fireEvent.click(editButtons[0]);
    // Edit mode shows a 取消 button in the card header.
    expect(screen.getByRole("button", { name: /^取消$/i })).toBeTruthy();
    // Click 取消 to exit edit mode.
    fireEvent.click(screen.getByRole("button", { name: /^取消$/i }));
    // Back in view mode — no 取消 button.
    expect(screen.queryByRole("button", { name: /^取消$/i })).toBeNull();
  });

  it("renders a ProviderCard for each provider", () => {
    renderPage({
      providers: [
        makeProvider({ id: "p1", name: "alpha" }),
        makeProvider({ id: "p2", name: "beta" }),
      ],
    });
    expect(screen.getByText("alpha")).toBeTruthy();
    expect(screen.getByText("beta")).toBeTruthy();
  });

  it("groups models by provider_id into the correct card", () => {
    renderPage({
      providers: [
        makeProvider({ id: "p1", name: "alpha" }),
        makeProvider({ id: "p2", name: "beta" }),
      ],
      models: [
        makeModel({ provider_id: "p1", model_id: "alpha-model" }),
        makeModel({ provider_id: "p2", model_id: "beta-model" }),
      ],
    });
    expect(screen.getByText("alpha-model")).toBeTruthy();
    expect(screen.getByText("beta-model")).toBeTruthy();
  });

  it("emits provider.deleted event on successful delete", async () => {
    renderPage({ providers: [makeProvider()] });
    // Click delete in the card → ConfirmDialog opens → click dialog confirm.
    fireEvent.click(screen.getByRole("button", { name: /删除/i }));
    const deleteButtons = screen.getAllByRole("button", { name: /删除/i });
    fireEvent.click(deleteButtons[deleteButtons.length - 1]);
    await waitFor(() => {
      expect(vi.mocked(reportFrontendEventSafely)).toHaveBeenCalledWith(
        expect.objectContaining({
          event_code: "provider.deleted",
          details: expect.objectContaining({ id: "prov-1" }),
        }),
      );
    });
  });

  it("emits provider.delete.failed event on failed delete and keeps dialog open with error", async () => {
    vi.mocked(useProviderMutations).mockReturnValue({
      ...mockMutations(),
      deleteProvider: {
        mutate: vi.fn(),
        mutateAsync: vi.fn().mockRejectedValue(new Error("delete rpc failed")),
        isPending: false,
      },
    } as never);
    renderPage({ providers: [makeProvider()] });
    // Click delete in the card → ConfirmDialog opens → click dialog confirm.
    fireEvent.click(screen.getByRole("button", { name: /删除/i }));
    const deleteButtons = screen.getAllByRole("button", { name: /删除/i });
    fireEvent.click(deleteButtons[deleteButtons.length - 1]);
    await waitFor(() => {
      expect(vi.mocked(reportFrontendEventSafely)).toHaveBeenCalledWith(
        expect.objectContaining({
          event_code: "provider.delete.failed",
          level: "ERROR",
          details: expect.objectContaining({ id: "prov-1" }),
        }),
      );
    });
    // P1 #2: dialog must stay open and surface the error (no silent close).
    expect(screen.getByText("删除 Provider")).toBeDefined();
    expect(screen.getByText(/delete rpc failed/)).toBeDefined();
  });

  it("emits provider.tested event on successful test connection", () => {
    renderPage({ providers: [makeProvider()] });
    fireEvent.click(screen.getByRole("button", { name: /测试连接/i }));
    expect(vi.mocked(reportFrontendEventSafely)).toHaveBeenCalledWith(
      expect.objectContaining({
        event_code: "provider.tested",
        details: expect.objectContaining({ id: "prov-1" }),
      }),
    );
  });

  it("emits provider.test.failed event when testConnection throws (client exception)", () => {
    // Override testConnection to trigger onError (client-side exception).
    vi.mocked(useProviderMutations).mockReturnValue({
      ...mockMutations(),
      testConnection: {
        mutate: vi.fn((_id: string, opts?: { onError?: (e: Error) => void }) => {
          opts?.onError?.(new Error("rpc timeout"));
        }),
        isPending: false,
      },
    } as never);
    renderPage({ providers: [makeProvider()] });
    fireEvent.click(screen.getByRole("button", { name: /测试连接/i }));
    expect(vi.mocked(reportFrontendEventSafely)).toHaveBeenCalledWith(
      expect.objectContaining({
        event_code: "provider.test.failed",
        level: "ERROR",
        details: expect.objectContaining({ id: "prov-1" }),
      }),
    );
  });

  it("emits model.added event on successful model create", async () => {
    renderPage({ providers: [makeProvider()] });
    fireEvent.click(screen.getByRole("button", { name: /\+ Add Model/i }));
    fireEvent.change(screen.getByPlaceholderText(/model name/i), {
      target: { value: "new-model" },
    });
    fireEvent.click(screen.getByRole("button", { name: /^保存$/i }));
    await new Promise((r) => setTimeout(r, 0));
    expect(vi.mocked(reportFrontendEventSafely)).toHaveBeenCalledWith(
      expect.objectContaining({
        event_code: "model.added",
        details: expect.objectContaining({
          provider_id: "prov-1",
          model_id: "new-model",
        }),
      }),
    );
  });

  it("emits model.add.failed event on failed model create", async () => {
    vi.mocked(useModelMutations).mockReturnValue({
      ...mockModelMutations(),
      createModel: {
        mutate: vi.fn(),
        mutateAsync: vi.fn().mockRejectedValue(new Error("model create rpc failed")),
        isPending: false,
      },
    } as never);
    renderPage({ providers: [makeProvider()] });
    fireEvent.click(screen.getByRole("button", { name: /\+ Add Model/i }));
    fireEvent.change(screen.getByPlaceholderText(/model name/i), {
      target: { value: "new-model" },
    });
    fireEvent.click(screen.getByRole("button", { name: /^保存$/i }));
    await new Promise((r) => setTimeout(r, 0));
    expect(vi.mocked(reportFrontendEventSafely)).toHaveBeenCalledWith(
      expect.objectContaining({
        event_code: "model.add.failed",
        level: "ERROR",
        details: expect.objectContaining({
          provider_id: "prov-1",
          model_id: "new-model",
        }),
      }),
    );
  });

  it("emits model.deleted event on successful model delete", async () => {
    renderPage({
      providers: [makeProvider()],
      models: [makeModel()],
    });
    // Click model row's delete → ConfirmDialog opens → click dialog confirm.
    let deleteButtons = screen.getAllByRole("button", { name: /删除/i });
    // Last delete button before dialog is the model row's.
    fireEvent.click(deleteButtons[deleteButtons.length - 1]);
    // Dialog confirm is now the last delete button.
    deleteButtons = screen.getAllByRole("button", { name: /删除/i });
    fireEvent.click(deleteButtons[deleteButtons.length - 1]);
    await waitFor(() => {
      expect(vi.mocked(reportFrontendEventSafely)).toHaveBeenCalledWith(
        expect.objectContaining({
          event_code: "model.deleted",
          details: expect.objectContaining({
            provider_id: "prov-1",
            model_id: "deepseek-chat",
          }),
        }),
      );
    });
  });

  it("emits model.delete.failed event on failed model delete and keeps dialog open with error", async () => {
    vi.mocked(useModelMutations).mockReturnValue({
      ...mockModelMutations(),
      deleteModel: {
        mutate: vi.fn(),
        mutateAsync: vi.fn().mockRejectedValue(new Error("model delete rpc failed")),
        isPending: false,
      },
    } as never);
    renderPage({
      providers: [makeProvider()],
      models: [makeModel()],
    });
    // Click model row's delete → ConfirmDialog opens → click dialog confirm.
    let deleteButtons = screen.getAllByRole("button", { name: /删除/i });
    fireEvent.click(deleteButtons[deleteButtons.length - 1]);
    deleteButtons = screen.getAllByRole("button", { name: /删除/i });
    fireEvent.click(deleteButtons[deleteButtons.length - 1]);
    await waitFor(() => {
      expect(vi.mocked(reportFrontendEventSafely)).toHaveBeenCalledWith(
        expect.objectContaining({
          event_code: "model.delete.failed",
          level: "ERROR",
          details: expect.objectContaining({
            provider_id: "prov-1",
            model_id: "deepseek-chat",
          }),
        }),
      );
    });
    // P1 #2: dialog must stay open and surface the error (no silent close).
    expect(screen.getByText("删除 Model")).toBeDefined();
    expect(screen.getByText(/model delete rpc failed/)).toBeDefined();
  });

  it("emits model.updated event on successful model update", async () => {
    renderPage({ providers: [makeProvider()], models: [makeModel()] });
    // Enter model edit mode, change display_name, save.
    const editButtons = screen.getAllByRole("button", { name: /编辑/i });
    fireEvent.click(editButtons[editButtons.length - 1]);
    fireEvent.change(screen.getByDisplayValue("deepseek-chat"), {
      target: { value: "DeepSeek Chat" },
    });
    fireEvent.click(screen.getByRole("button", { name: /^保存$/i }));
    await new Promise((r) => setTimeout(r, 0));
    expect(vi.mocked(reportFrontendEventSafely)).toHaveBeenCalledWith(
      expect.objectContaining({
        event_code: "model.updated",
        level: "INFO",
        details: expect.objectContaining({
          provider_id: "prov-1",
          model_id: "deepseek-chat",
        }),
      }),
    );
  });

  it("emits model.update.failed event on failed model update", async () => {
    vi.mocked(useModelMutations).mockReturnValue({
      ...mockModelMutations(),
      updateModel: {
        mutate: vi.fn(),
        mutateAsync: vi.fn().mockRejectedValue(new Error("update failed")),
        isPending: false,
      },
    } as never);
    renderPage({ providers: [makeProvider()], models: [makeModel()] });
    const editButtons = screen.getAllByRole("button", { name: /编辑/i });
    fireEvent.click(editButtons[editButtons.length - 1]);
    fireEvent.change(screen.getByDisplayValue("deepseek-chat"), {
      target: { value: "DeepSeek Chat" },
    });
    fireEvent.click(screen.getByRole("button", { name: /^保存$/i }));
    await new Promise((r) => setTimeout(r, 0));
    expect(vi.mocked(reportFrontendEventSafely)).toHaveBeenCalledWith(
      expect.objectContaining({
        event_code: "model.update.failed",
        level: "ERROR",
        details: expect.objectContaining({
          provider_id: "prov-1",
          model_id: "deepseek-chat",
        }),
      }),
    );
  });

  it("emits model.tags.updated event on successful tags update", async () => {
    const mutateAsyncSpy = vi.fn().mockResolvedValue(undefined);
    vi.mocked(useModelMutations).mockReturnValue({
      ...mockModelMutations(),
      tagsUpdate: { mutate: vi.fn(), mutateAsync: mutateAsyncSpy, isPending: false },
    } as never);
    renderPage({ providers: [makeProvider()], models: [makeModel()] });
    const editButtons = screen.getAllByRole("button", { name: /编辑/i });
    fireEvent.click(editButtons[editButtons.length - 1]);
    fireEvent.change(screen.getByPlaceholderText(/tags/i), {
      target: { value: "cheap,fast" },
    });
    fireEvent.click(screen.getByRole("button", { name: /^保存$/i }));
    await new Promise((r) => setTimeout(r, 0));
    // Regression (C1): mutateAsync must receive the SQL PK (model_db_id), not
    // the human-readable model_id string. makeModel() sets them distinctly.
    expect(mutateAsyncSpy).toHaveBeenCalledWith(
      { modelId: "model-db-1", tags: ["cheap", "fast"] },
    );
    expect(vi.mocked(reportFrontendEventSafely)).toHaveBeenCalledWith(
      expect.objectContaining({
        event_code: "model.tags.updated",
        level: "INFO",
        details: expect.objectContaining({
          provider_id: "prov-1",
          model_id: "deepseek-chat",
        }),
      }),
    );
  });

  it("emits model.tags.update.failed event on failed tags update", async () => {
    const mutateAsyncSpy = vi.fn().mockRejectedValue(new Error("tags update failed"));
    vi.mocked(useModelMutations).mockReturnValue({
      ...mockModelMutations(),
      tagsUpdate: { mutate: vi.fn(), mutateAsync: mutateAsyncSpy, isPending: false },
    } as never);
    renderPage({ providers: [makeProvider()], models: [makeModel()] });
    const editButtons = screen.getAllByRole("button", { name: /编辑/i });
    fireEvent.click(editButtons[editButtons.length - 1]);
    fireEvent.change(screen.getByPlaceholderText(/tags/i), {
      target: { value: "cheap,fast" },
    });
    fireEvent.click(screen.getByRole("button", { name: /^保存$/i }));
    await new Promise((r) => setTimeout(r, 0));
    // Regression (C1): same payload assertion as the success case.
    expect(mutateAsyncSpy).toHaveBeenCalledWith(
      { modelId: "model-db-1", tags: ["cheap", "fast"] },
    );
    expect(vi.mocked(reportFrontendEventSafely)).toHaveBeenCalledWith(
      expect.objectContaining({
        event_code: "model.tags.update.failed",
        level: "ERROR",
        details: expect.objectContaining({
          provider_id: "prov-1",
          model_id: "deepseek-chat",
        }),
      }),
    );
  });

  it("shows error banner when providers query fails", () => {
    vi.mocked(useProviders).mockReturnValue({
      data: undefined,
      isLoading: false,
      isError: true,
      isFetching: false,
    } as never);
    vi.mocked(useModels).mockReturnValue({
      data: { models: [] } as ModelListResponseDto,
      isLoading: false,
      isError: false,
      isFetching: false,
    } as never);
    const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(
      <QueryClientProvider client={qc}>
        <ProvidersPage />
      </QueryClientProvider>,
    );
    expect(screen.getByText(/Provider 列表加载失败/i)).toBeTruthy();
  });

  it("shows error banner when models query fails", () => {
    vi.mocked(useProviders).mockReturnValue({
      data: { providers: [makeProvider()] } as ProviderListResponseDto,
      isLoading: false,
      isError: false,
      isFetching: false,
    } as never);
    vi.mocked(useModels).mockReturnValue({
      data: undefined,
      isLoading: false,
      isError: true,
      isFetching: false,
    } as never);
    const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(
      <QueryClientProvider client={qc}>
        <ProvidersPage />
      </QueryClientProvider>,
    );
    expect(screen.getByText(/Model 列表加载失败/i)).toBeTruthy();
  });

  it("renders H1 heading for the provider catalog (f7)", () => {
    renderPage();
    expect(screen.getByRole("heading", { level: 1, name: /providers/i })).toBeTruthy();
  });

  it("surfaces test-connection success result in the card UI (f3)", () => {
    renderPage({ providers: [makeProvider()] });
    fireEvent.click(screen.getByRole("button", { name: /测试连接/i }));
    expect(screen.getByText("连接成功")).toBeTruthy();
  });

  it("surfaces test-connection failure result in the card UI (f3)", () => {
    vi.mocked(useProviderMutations).mockReturnValue({
      ...mockMutations(),
      testConnection: {
        mutate: vi.fn((_id: string, opts?: { onSuccess?: (r: unknown) => void }) => {
          opts?.onSuccess?.({ ok: false, error: "connection refused", models_detected: null });
        }),
        isPending: false,
      },
    } as never);
    renderPage({ providers: [makeProvider()] });
    fireEvent.click(screen.getByRole("button", { name: /测试连接/i }));
    expect(screen.getByText(/连接失败/)).toBeTruthy();
    expect(screen.getByText(/connection refused/)).toBeTruthy();
  });
});
