import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, render, screen, fireEvent } from "@testing-library/react";
import type { ProviderDto, ModelCatalogEntryDto, ModelCreateRequestDto, ModelUpdateRequestDto } from "@busytok/protocol-types";
import { ProviderCard } from "./ProviderCard";

afterEach(() => cleanup());

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

const noopMutations = {
  createProvider: { mutate: vi.fn(), isPending: false },
  updateProvider: { mutate: vi.fn(), isPending: false },
  deleteProvider: { mutate: vi.fn(), isPending: false },
  testConnection: { mutate: vi.fn(), isPending: false },
} as never;

const noopModelMutations = {
  createModel: { mutate: vi.fn(), isPending: false },
  updateModel: { mutate: vi.fn(), isPending: false },
  deleteModel: { mutate: vi.fn(), isPending: false },
  tagsUpdate: { mutate: vi.fn(), isPending: false },
} as never;

describe("ProviderCard (view mode)", () => {
  it("renders provider name, kind chip, and base url", () => {
    render(
      <ProviderCard
        provider={makeProvider()}
        models={[]}
        isModelsLoading={false}
        providerMutations={noopMutations}
        modelMutations={noopModelMutations}
        onEdit={vi.fn()}
        onTestConnection={vi.fn()}
        onDelete={vi.fn()}
        onModelCreate={vi.fn()}
        onModelUpdate={vi.fn()}
        onModelTagsUpdate={vi.fn()}
        onModelDelete={vi.fn()}
      />,
    );
    expect(screen.getByText("deepseek_openai")).toBeDefined();
    expect(screen.getByText("openai")).toBeDefined();
    expect(screen.getByText("https://api.deepseek.com/v1")).toBeDefined();
  });

  it("renders provider id in monospace", () => {
    render(
      <ProviderCard
        provider={makeProvider({ id: "abc-123" })}
        models={[]}
        isModelsLoading={false}
        providerMutations={noopMutations}
        modelMutations={noopModelMutations}
        onEdit={vi.fn()}
        onTestConnection={vi.fn()}
        onDelete={vi.fn()}
        onModelCreate={vi.fn()}
        onModelUpdate={vi.fn()}
        onModelTagsUpdate={vi.fn()}
        onModelDelete={vi.fn()}
      />,
    );
    expect(screen.getByText(/abc-123/)).toBeDefined();
  });

  it("renders model rows when models are provided", () => {
    render(
      <ProviderCard
        provider={makeProvider()}
        models={[
          makeModel({ model_db_id: "model-db-1", model_id: "deepseek-chat" }),
          makeModel({ model_db_id: "model-db-2", model_id: "deepseek-reason" }),
        ]}
        isModelsLoading={false}
        providerMutations={noopMutations}
        modelMutations={noopModelMutations}
        onEdit={vi.fn()}
        onTestConnection={vi.fn()}
        onDelete={vi.fn()}
        onModelCreate={vi.fn()}
        onModelUpdate={vi.fn()}
        onModelTagsUpdate={vi.fn()}
        onModelDelete={vi.fn()}
      />,
    );
    expect(screen.getByText("deepseek-chat")).toBeDefined();
    expect(screen.getByText("deepseek-reason")).toBeDefined();
  });

  it("renders tags as chips", () => {
    render(
      <ProviderCard
        provider={makeProvider()}
        models={[makeModel({ tags: ["cheap", "fast"] })]}
        isModelsLoading={false}
        providerMutations={noopMutations}
        modelMutations={noopModelMutations}
        onEdit={vi.fn()}
        onTestConnection={vi.fn()}
        onDelete={vi.fn()}
        onModelCreate={vi.fn()}
        onModelUpdate={vi.fn()}
        onModelTagsUpdate={vi.fn()}
        onModelDelete={vi.fn()}
      />,
    );
    expect(screen.getByText("cheap")).toBeDefined();
    expect(screen.getByText("fast")).toBeDefined();
  });

  it("renders empty-state message when models list is empty and not loading", () => {
    render(
      <ProviderCard
        provider={makeProvider()}
        models={[]}
        isModelsLoading={false}
        providerMutations={noopMutations}
        modelMutations={noopModelMutations}
        onEdit={vi.fn()}
        onTestConnection={vi.fn()}
        onDelete={vi.fn()}
        onModelCreate={vi.fn()}
        onModelUpdate={vi.fn()}
        onModelTagsUpdate={vi.fn()}
        onModelDelete={vi.fn()}
      />,
    );
    expect(screen.getByText(/暂无 model/)).toBeDefined();
  });

  it("renders loading state when isModelsLoading is true", () => {
    render(
      <ProviderCard
        provider={makeProvider()}
        models={[]}
        isModelsLoading={true}
        providerMutations={noopMutations}
        modelMutations={noopModelMutations}
        onEdit={vi.fn()}
        onTestConnection={vi.fn()}
        onDelete={vi.fn()}
        onModelCreate={vi.fn()}
        onModelUpdate={vi.fn()}
        onModelTagsUpdate={vi.fn()}
        onModelDelete={vi.fn()}
      />,
    );
    expect(screen.getByText(/加载中/)).toBeDefined();
  });

  it("calls onEdit when Edit button clicked", () => {
    const onEdit = vi.fn();
    render(
      <ProviderCard
        provider={makeProvider()}
        models={[]}
        isModelsLoading={false}
        providerMutations={noopMutations}
        modelMutations={noopModelMutations}
        onEdit={onEdit}
        onTestConnection={vi.fn()}
        onDelete={vi.fn()}
        onModelCreate={vi.fn()}
        onModelUpdate={vi.fn()}
        onModelTagsUpdate={vi.fn()}
        onModelDelete={vi.fn()}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: /编辑/i }));
    expect(onEdit).toHaveBeenCalledOnce();
  });

  it("calls onTestConnection when Test button clicked", () => {
    const onTestConnection = vi.fn();
    render(
      <ProviderCard
        provider={makeProvider()}
        models={[]}
        isModelsLoading={false}
        providerMutations={noopMutations}
        modelMutations={noopModelMutations}
        onEdit={vi.fn()}
        onTestConnection={onTestConnection}
        onDelete={vi.fn()}
        onModelCreate={vi.fn()}
        onModelUpdate={vi.fn()}
        onModelTagsUpdate={vi.fn()}
        onModelDelete={vi.fn()}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: /测试连接/i }));
    expect(onTestConnection).toHaveBeenCalledWith("prov-1");
  });

  it("calls onDelete when Delete button clicked and user confirms", () => {
    const confirmSpy = vi.spyOn(globalThis, "confirm").mockReturnValue(true);
    const onDelete = vi.fn();
    render(
      <ProviderCard
        provider={makeProvider()}
        models={[]}
        isModelsLoading={false}
        providerMutations={noopMutations}
        modelMutations={noopModelMutations}
        onEdit={vi.fn()}
        onTestConnection={vi.fn()}
        onDelete={onDelete}
        onModelCreate={vi.fn()}
        onModelUpdate={vi.fn()}
        onModelTagsUpdate={vi.fn()}
        onModelDelete={vi.fn()}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: /删除/i }));
    expect(confirmSpy).toHaveBeenCalled();
    expect(onDelete).toHaveBeenCalledWith(makeProvider());
    confirmSpy.mockRestore();
  });

  it("does not call onDelete when user cancels confirm", () => {
    const confirmSpy = vi.spyOn(globalThis, "confirm").mockReturnValue(false);
    const onDelete = vi.fn();
    render(
      <ProviderCard
        provider={makeProvider()}
        models={[]}
        isModelsLoading={false}
        providerMutations={noopMutations}
        modelMutations={noopModelMutations}
        onEdit={vi.fn()}
        onTestConnection={vi.fn()}
        onDelete={onDelete}
        onModelCreate={vi.fn()}
        onModelUpdate={vi.fn()}
        onModelTagsUpdate={vi.fn()}
        onModelDelete={vi.fn()}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: /删除/i }));
    expect(onDelete).not.toHaveBeenCalled();
    confirmSpy.mockRestore();
  });
});

describe("ProviderCard model create", () => {
  it("shows inline create form when + Add Model clicked", () => {
    render(
      <ProviderCard
        provider={makeProvider()}
        models={[]}
        isModelsLoading={false}
        providerMutations={noopMutations}
        modelMutations={noopModelMutations}
        onEdit={vi.fn()}
        onTestConnection={vi.fn()}
        onDelete={vi.fn()}
        onModelCreate={vi.fn()}
        onModelUpdate={vi.fn()}
        onModelTagsUpdate={vi.fn()}
        onModelDelete={vi.fn()}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: /\+ Add Model/i }));
    expect(screen.getByPlaceholderText(/model name/i)).toBeDefined();
  });

  it("calls onModelCreate with derived payload on Save", () => {
    const onModelCreate = vi.fn();
    render(
      <ProviderCard
        provider={makeProvider()}
        models={[]}
        isModelsLoading={false}
        providerMutations={noopMutations}
        modelMutations={noopModelMutations}
        onEdit={vi.fn()}
        onTestConnection={vi.fn()}
        onDelete={vi.fn()}
        onModelCreate={onModelCreate}
        onModelUpdate={vi.fn()}
        onModelTagsUpdate={vi.fn()}
        onModelDelete={vi.fn()}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: /\+ Add Model/i }));
    fireEvent.change(screen.getByPlaceholderText(/model name/i), { target: { value: "deepseek-chat" } });
    fireEvent.click(screen.getByRole("button", { name: /^保存$/i }));
    const expected: ModelCreateRequestDto = {
      provider_id: "prov-1",
      model_id: "deepseek-chat",
      display_name: "deepseek-chat",
      context_window: 200000,
      max_tokens: 8192,
      reasoning: true,
      enabled: true,
      tags: [],
    };
    expect(onModelCreate).toHaveBeenCalledWith(expected);
  });

  it("parses tags from comma-separated input", () => {
    const onModelCreate = vi.fn();
    render(
      <ProviderCard
        provider={makeProvider()}
        models={[]}
        isModelsLoading={false}
        providerMutations={noopMutations}
        modelMutations={noopModelMutations}
        onEdit={vi.fn()}
        onTestConnection={vi.fn()}
        onDelete={vi.fn()}
        onModelCreate={onModelCreate}
        onModelUpdate={vi.fn()}
        onModelTagsUpdate={vi.fn()}
        onModelDelete={vi.fn()}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: /\+ Add Model/i }));
    fireEvent.change(screen.getByPlaceholderText(/model name/i), { target: { value: "deepseek-chat" } });
    fireEvent.change(screen.getByPlaceholderText(/tags/i), { target: { value: "cheap, fast" } });
    fireEvent.click(screen.getByRole("button", { name: /^保存$/i }));
    expect(onModelCreate).toHaveBeenCalledWith(
      expect.objectContaining({ tags: ["cheap", "fast"] }),
    );
  });
});

describe("ProviderCard model delete", () => {
  it("calls onModelDelete after confirm", () => {
    const confirmSpy = vi.spyOn(globalThis, "confirm").mockReturnValue(true);
    const onModelDelete = vi.fn();
    render(
      <ProviderCard
        provider={makeProvider()}
        models={[makeModel()]}
        isModelsLoading={false}
        providerMutations={noopMutations}
        modelMutations={noopModelMutations}
        onEdit={vi.fn()}
        onTestConnection={vi.fn()}
        onDelete={vi.fn()}
        onModelCreate={vi.fn()}
        onModelUpdate={vi.fn()}
        onModelTagsUpdate={vi.fn()}
        onModelDelete={onModelDelete}
      />,
    );
    // The model row's 删除 button is the last 删除 in the DOM (provider delete
    // is in the header, rendered before the model rows).
    const deleteButtons = screen.getAllByRole("button", { name: /删除/i });
    fireEvent.click(deleteButtons[deleteButtons.length - 1]);
    expect(onModelDelete).toHaveBeenCalledWith(makeModel());
    confirmSpy.mockRestore();
  });
});

describe("ProviderCard model edit", () => {
  const renderCardWithModel = (overrides: { onModelUpdate?: ReturnType<typeof vi.fn>; onModelTagsUpdate?: ReturnType<typeof vi.fn> } = {}) => {
    const onModelUpdate = overrides.onModelUpdate ?? vi.fn();
    const onModelTagsUpdate = overrides.onModelTagsUpdate ?? vi.fn();
    const result = render(
      <ProviderCard
        provider={makeProvider()}
        models={[makeModel()]}
        isModelsLoading={false}
        providerMutations={noopMutations}
        modelMutations={noopModelMutations}
        onEdit={vi.fn()}
        onTestConnection={vi.fn()}
        onDelete={vi.fn()}
        onModelCreate={vi.fn()}
        onModelUpdate={onModelUpdate}
        onModelTagsUpdate={onModelTagsUpdate}
        onModelDelete={vi.fn()}
      />,
    );
    return { ...result, onModelUpdate, onModelTagsUpdate };
  };

  it("shows edit form with current model values when 编辑 clicked", () => {
    renderCardWithModel();
    // Click the model-row 编辑 button (the last one — provider 编辑 is in the header).
    const editButtons = screen.getAllByRole("button", { name: /编辑/i });
    fireEvent.click(editButtons[editButtons.length - 1]);
    // Edit form should show current display_name, context_window, max_tokens, reasoning.
    expect(screen.getByDisplayValue("deepseek-chat")).toBeDefined(); // display_name
    expect(screen.getByDisplayValue("200000")).toBeDefined(); // context_window
    expect(screen.getByDisplayValue("8192")).toBeDefined(); // max_tokens
    // makeModel has reasoning: false.
    expect((screen.getByRole("checkbox", { name: /reasoning/i }) as HTMLInputElement).checked).toBe(false);
  });

  it("calls onModelUpdate with only changed fields on save (single-state Option semantics)", () => {
    const { onModelUpdate } = renderCardWithModel();
    const editButtons = screen.getAllByRole("button", { name: /编辑/i });
    fireEvent.click(editButtons[editButtons.length - 1]);
    // Change only display_name; leave other fields unchanged.
    const nameInput = screen.getByDisplayValue("deepseek-chat");
    fireEvent.change(nameInput, { target: { value: "DeepSeek Chat" } });
    fireEvent.click(screen.getByRole("button", { name: /^保存$/i }));
    // First arg is the model being edited; second is the patch with id + only changed field.
    expect(onModelUpdate).toHaveBeenCalledWith(
      makeModel(),
      expect.objectContaining({
        id: "model-db-1",
        display_name: "DeepSeek Chat",
      }),
    );
    // Unchanged fields must NOT be in the patch (omit = no change per ModelUpdateRequestDto semantics).
    const call = onModelUpdate.mock.calls[0][1] as ModelUpdateRequestDto;
    expect(call.context_window).toBeUndefined();
    expect(call.max_tokens).toBeUndefined();
    expect(call.reasoning).toBeUndefined();
    expect(call.enabled).toBeUndefined();
  });

  it("calls onModelTagsUpdate when tags changed on save", () => {
    const { onModelTagsUpdate } = renderCardWithModel();
    const editButtons = screen.getAllByRole("button", { name: /编辑/i });
    fireEvent.click(editButtons[editButtons.length - 1]);
    // Change the tags input (empty in makeModel → set to "cheap,fast").
    const tagsInput = screen.getByPlaceholderText(/tags/i);
    fireEvent.change(tagsInput, { target: { value: "cheap,fast" } });
    fireEvent.click(screen.getByRole("button", { name: /^保存$/i }));
    expect(onModelTagsUpdate).toHaveBeenCalledWith(makeModel(), ["cheap", "fast"]);
  });

  it("does not call onModelTagsUpdate when tags unchanged", () => {
    const { onModelTagsUpdate } = renderCardWithModel({
      onModelTagsUpdate: vi.fn(),
    });
    const editButtons = screen.getAllByRole("button", { name: /编辑/i });
    fireEvent.click(editButtons[editButtons.length - 1]);
    // Don't touch tags — leave them as-is (empty in makeModel).
    fireEvent.click(screen.getByRole("button", { name: /^保存$/i }));
    expect(onModelTagsUpdate).not.toHaveBeenCalled();
  });

  it("exits edit mode on Cancel", () => {
    renderCardWithModel();
    const editButtons = screen.getAllByRole("button", { name: /编辑/i });
    fireEvent.click(editButtons[editButtons.length - 1]);
    fireEvent.click(screen.getByRole("button", { name: /取消/i }));
    // Edit form should be gone; the model row's view mode should show again.
    expect(screen.queryByRole("checkbox", { name: /reasoning/i })).toBeNull();
  });

  it("does not clear display_name when edit input is empty (single-state Option semantics)", () => {
    const { onModelUpdate } = renderCardWithModel();
    const editButtons = screen.getAllByRole("button", { name: /编辑/i });
    fireEvent.click(editButtons[editButtons.length - 1]);
    // Clear the display_name input to empty string.
    const nameInput = screen.getByDisplayValue("deepseek-chat");
    fireEvent.change(nameInput, { target: { value: "" } });
    fireEvent.click(screen.getByRole("button", { name: /^保存$/i }));
    // onModelUpdate should NOT be called because the only changed field (display_name)
    // is empty → treated as "leave unchanged" → patch has only {id} → no update sent.
    expect(onModelUpdate).not.toHaveBeenCalled();
  });
});

describe("ProviderCard edit mode", () => {
  // Render the card in edit mode with a default updateProvider mock that
  // invokes the onSuccess callback (so the component's success path runs).
  // The mock is exposed so each test can assert on its calls.
  const renderCardInEditMode = (overrides: { updateProvider?: ReturnType<typeof vi.fn>; onCancelEdit?: ReturnType<typeof vi.fn> } = {}) => {
    const updateProvider = overrides.updateProvider ?? vi.fn((_payload: unknown, opts?: { onSuccess?: () => void }) => {
      opts?.onSuccess?.();
    });
    const onCancelEdit = overrides.onCancelEdit ?? vi.fn();
    const providerMutations = {
      createProvider: { mutate: vi.fn(), isPending: false },
      updateProvider: { mutate: updateProvider, isPending: false },
      deleteProvider: { mutate: vi.fn(), isPending: false },
      testConnection: { mutate: vi.fn(), isPending: false },
    } as never;
    render(
      <ProviderCard
        provider={makeProvider()}
        models={[makeModel()]}
        isModelsLoading={false}
        providerMutations={providerMutations}
        modelMutations={noopModelMutations}
        onEdit={vi.fn()}
        onTestConnection={vi.fn()}
        onDelete={vi.fn()}
        onModelCreate={vi.fn()}
        onModelUpdate={vi.fn()}
        onModelTagsUpdate={vi.fn()}
        onModelDelete={vi.fn()}
        isEditing={true}
        onCancelEdit={onCancelEdit}
      />,
    );
    return { updateProvider, onCancelEdit };
  };

  it("renders editable inputs for base url, api key, kind, name when isEditing", () => {
    renderCardInEditMode();
    expect(screen.getByDisplayValue("https://api.deepseek.com/v1")).toBeDefined();
    expect(screen.getByDisplayValue("deepseek_openai")).toBeDefined();
    // API key field shows placeholder for new key
    expect(screen.getByPlaceholderText(/new api key/i)).toBeDefined();
  });

  it("disables model operation buttons when editing", () => {
    renderCardInEditMode();
    const addModelButton = screen.getByRole("button", { name: /\+ Add Model/i }) as HTMLButtonElement;
    expect(addModelButton.disabled).toBe(true);
  });

  it("shows notice when editing", () => {
    renderCardInEditMode();
    expect(screen.getByText(/正在编辑 Provider 信息/i)).toBeDefined();
  });

  it("calls updateProvider with patch on Save", () => {
    const { updateProvider } = renderCardInEditMode();
    fireEvent.change(screen.getByDisplayValue("https://api.deepseek.com/v1"), { target: { value: "https://api.deepseek.com/v2" } });
    fireEvent.click(screen.getByRole("button", { name: /^保存$/i }));
    expect(updateProvider).toHaveBeenCalledWith(
      expect.objectContaining({
        id: "prov-1",
        base_url: "https://api.deepseek.com/v2",
      }),
      expect.anything(),
    );
  });

  it("omits api_key from patch when key field is empty (no change)", () => {
    const { updateProvider } = renderCardInEditMode();
    fireEvent.click(screen.getByRole("button", { name: /^保存$/i }));
    const call = (updateProvider as unknown as { mock: { calls: unknown[][] } }).mock.calls[0][0] as Record<string, unknown>;
    expect(call.api_key).toBeUndefined();
  });

  it("includes api_key in patch when key field is filled", () => {
    const { updateProvider } = renderCardInEditMode();
    fireEvent.change(screen.getByPlaceholderText(/new api key/i), { target: { value: "sk-new" } });
    fireEvent.click(screen.getByRole("button", { name: /^保存$/i }));
    const call = (updateProvider as unknown as { mock: { calls: unknown[][] } }).mock.calls[0][0] as Record<string, unknown>;
    expect(call.api_key).toBe("sk-new");
  });

  it("calls onCancelEdit on Cancel button click", () => {
    const onCancelEdit = vi.fn();
    renderCardInEditMode({ onCancelEdit });
    fireEvent.click(screen.getByRole("button", { name: /^取消$/i }));
    expect(onCancelEdit).toHaveBeenCalledOnce();
  });
});
