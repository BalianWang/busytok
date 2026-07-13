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

/** Minimal props for view-mode rendering. */
const defaultProps = (overrides: Record<string, unknown> = {}) => ({
  provider: makeProvider(),
  models: [] as ModelCatalogEntryDto[],
  isModelsLoading: false,
  providerMutations: noopMutations,
  onEdit: vi.fn(),
  onTestConnection: vi.fn(),
  onDelete: vi.fn(),
  onModelCreate: vi.fn(),
  onModelUpdate: vi.fn(),
  onModelTagsUpdate: vi.fn(),
  onModelDelete: vi.fn(),
  ...overrides,
});

describe("ProviderCard (view mode)", () => {
  it("renders provider name, kind chip, and base url", () => {
    render(<ProviderCard {...defaultProps()} />);
    expect(screen.getByText("deepseek_openai")).toBeDefined();
    expect(screen.getByText("OpenAI-compatible")).toBeDefined();
    expect(screen.getByText("https://api.deepseek.com/v1")).toBeDefined();
  });

  it("renders provider id", () => {
    render(<ProviderCard {...defaultProps({ provider: makeProvider({ id: "abc-123" }) })} />);
    expect(screen.getByText(/abc-123/)).toBeDefined();
  });

  it("renders model rows when models are provided", () => {
    render(
      <ProviderCard
        {...defaultProps({
          models: [
            makeModel({ model_db_id: "model-db-1", model_id: "deepseek-chat" }),
            makeModel({ model_db_id: "model-db-2", model_id: "deepseek-reason" }),
          ],
        })}
      />,
    );
    expect(screen.getByText("deepseek-chat")).toBeDefined();
    expect(screen.getByText("deepseek-reason")).toBeDefined();
  });

  it("renders tags as chips", () => {
    render(<ProviderCard {...defaultProps({ models: [makeModel({ tags: ["cheap", "fast"] })] })} />);
    expect(screen.getByText("cheap")).toBeDefined();
    expect(screen.getByText("fast")).toBeDefined();
  });

  it("renders empty-state message when models list is empty and not loading", () => {
    render(<ProviderCard {...defaultProps()} />);
    expect(screen.getByText(/No models/)).toBeDefined();
  });

  it("renders loading state when isModelsLoading is true", () => {
    render(<ProviderCard {...defaultProps({ isModelsLoading: true })} />);
    expect(screen.getByText(/Loading/)).toBeDefined();
  });

  it("calls onEdit when Edit button clicked", () => {
    const onEdit = vi.fn();
    render(<ProviderCard {...defaultProps({ onEdit })} />);
    fireEvent.click(screen.getByRole("button", { name: /Edit/i }));
    expect(onEdit).toHaveBeenCalledOnce();
  });

  it("calls onTestConnection when Test button clicked", () => {
    const onTestConnection = vi.fn();
    render(<ProviderCard {...defaultProps({ onTestConnection })} />);
    fireEvent.click(screen.getByRole("button", { name: /Test Connection/i }));
    expect(onTestConnection).toHaveBeenCalledWith("prov-1");
  });

  it("renders disabled indicator when provider.enabled is false", () => {
    render(<ProviderCard {...defaultProps({ provider: makeProvider({ enabled: false }) })} />);
    expect(screen.getByText(/Disabled/)).toBeDefined();
  });

  it("renders raw provider_kind when kind is not in KIND_LABEL", () => {
    render(
      <ProviderCard
        {...defaultProps({ provider: makeProvider({ provider_kind: "custom_kind" as never }) })}
      />,
    );
    expect(screen.getByText("custom_kind")).toBeDefined();
  });

  it("renders disabled indicator when model.model_enabled is false", () => {
    render(
      <ProviderCard
        {...defaultProps({ models: [makeModel({ model_enabled: false })] })}
      />,
    );
    expect(screen.getByText(/Disabled/)).toBeDefined();
  });
});

// ─── Model with null/undefined optional fields (toEditDraft defaults) ─────
describe("ProviderCard model edit with null optional fields", () => {
  it("applies default values when model optional fields are null/undefined", () => {
    const model = makeModel({
      display_name: undefined as never,
      context_window: undefined as never,
      max_tokens: undefined as never,
      reasoning: undefined as never,
    });
    render(<ProviderCard {...defaultProps({ models: [model] })} />);
    const editButtons = screen.getAllByRole("button", { name: /Edit/i });
    fireEvent.click(editButtons[editButtons.length - 1]);
    // display_name defaults to "" → input is empty
    expect((screen.getByLabelText(/display name/i) as HTMLInputElement).value).toBe("");
    // context_window defaults to 200000
    expect((screen.getByLabelText(/context window/i) as HTMLInputElement).value).toBe("200000");
    // max_tokens defaults to 8192
    expect((screen.getByLabelText(/max tokens/i) as HTMLInputElement).value).toBe("8192");
    // reasoning defaults to false
    expect((screen.getByRole("checkbox", { name: /reasoning/i }) as HTMLInputElement).checked).toBe(false);
  });
});

// ─── ConfirmDialog integration (f1) ─────────────────────────────────────
describe("ProviderCard delete via ConfirmDialog", () => {
  it("opens confirm dialog with provider delete content when Delete clicked", () => {
    render(<ProviderCard {...defaultProps()} />);
    fireEvent.click(screen.getByRole("button", { name: /Delete/i }));
    expect(screen.getByText("Delete Provider")).toBeDefined();
    expect(screen.getByText(/Delete provider "deepseek_openai"/)).toBeDefined();
  });

  it("calls onDelete when dialog confirm clicked", () => {
    const onDelete = vi.fn();
    render(<ProviderCard {...defaultProps({ onDelete })} />);
    fireEvent.click(screen.getByRole("button", { name: /Delete/i }));
    // Dialog confirm button is the last Delete button after dialog opens.
    const deleteButtons = screen.getAllByRole("button", { name: /Delete/i });
    fireEvent.click(deleteButtons[deleteButtons.length - 1]);
    expect(onDelete).toHaveBeenCalledWith(makeProvider());
  });

  it("does not call onDelete when dialog cancelled", () => {
    const onDelete = vi.fn();
    render(<ProviderCard {...defaultProps({ onDelete })} />);
    fireEvent.click(screen.getByRole("button", { name: /Delete/i }));
    fireEvent.click(screen.getByRole("button", { name: /cancel/i }));
    expect(onDelete).not.toHaveBeenCalled();
  });

  it("opens confirm dialog with model delete content when model Delete clicked", () => {
    render(<ProviderCard {...defaultProps({ models: [makeModel()] })} />);
    const deleteButtons = screen.getAllByRole("button", { name: /Delete/i });
    // Click the model row's Delete (last one before dialog opens).
    fireEvent.click(deleteButtons[deleteButtons.length - 1]);
    expect(screen.getByText("Delete Model")).toBeDefined();
    expect(screen.getByText(/Delete model "deepseek-chat"/)).toBeDefined();
  });

  it("calls onModelDelete when dialog confirm clicked", () => {
    const onModelDelete = vi.fn();
    render(<ProviderCard {...defaultProps({ models: [makeModel()], onModelDelete })} />);
    const deleteButtons = screen.getAllByRole("button", { name: /Delete/i });
    fireEvent.click(deleteButtons[deleteButtons.length - 1]);
    // Dialog confirm is now the last Delete button.
    const dialogDeleteButtons = screen.getAllByRole("button", { name: /Delete/i });
    fireEvent.click(dialogDeleteButtons[dialogDeleteButtons.length - 1]);
    expect(onModelDelete).toHaveBeenCalledWith(makeModel());
  });

  it("does not call onModelDelete when dialog cancelled", () => {
    const onModelDelete = vi.fn();
    render(<ProviderCard {...defaultProps({ models: [makeModel()], onModelDelete })} />);
    const deleteButtons = screen.getAllByRole("button", { name: /Delete/i });
    fireEvent.click(deleteButtons[deleteButtons.length - 1]);
    fireEvent.click(screen.getByRole("button", { name: /cancel/i }));
    expect(onModelDelete).not.toHaveBeenCalled();
  });
});

// ─── Test connection result (f3) ─────────────────────────────────────────
describe("ProviderCard test result display", () => {
  it("renders success result when testResult.ok is true", () => {
    render(<ProviderCard {...defaultProps({ testResult: { ok: true, error: null } })} />);
    expect(screen.getByText("Connection successful")).toBeDefined();
  });

  it("renders failure result with error message when testResult.ok is false", () => {
    render(<ProviderCard {...defaultProps({ testResult: { ok: false, error: "timeout" } })} />);
    expect(screen.getByText(/Connection failed/)).toBeDefined();
    expect(screen.getByText(/timeout/)).toBeDefined();
  });

  it("does not render test result section when testResult is undefined", () => {
    render(<ProviderCard {...defaultProps()} />);
    expect(screen.queryByText(/Connection successful/)).toBeNull();
    expect(screen.queryByText(/Connection failed/)).toBeNull();
  });
});

// ─── Disable on pending (f4) ──────────────────────────────────────────────
describe("ProviderCard disable on mutation pending", () => {
  it("disables action buttons when updateProvider.isPending", () => {
    const pendingMutations = {
      createProvider: { mutate: vi.fn(), isPending: false },
      updateProvider: { mutate: vi.fn(), isPending: true },
      deleteProvider: { mutate: vi.fn(), isPending: false },
      testConnection: { mutate: vi.fn(), isPending: false },
    } as never;
    render(<ProviderCard {...defaultProps({ providerMutations: pendingMutations })} />);
    expect((screen.getByRole("button", { name: /Edit/i }) as HTMLButtonElement).disabled).toBe(true);
    expect((screen.getByRole("button", { name: /Delete/i }) as HTMLButtonElement).disabled).toBe(true);
  });

  it("disables test connection button when testConnection.isPending", () => {
    const pendingMutations = {
      createProvider: { mutate: vi.fn(), isPending: false },
      updateProvider: { mutate: vi.fn(), isPending: false },
      deleteProvider: { mutate: vi.fn(), isPending: false },
      testConnection: { mutate: vi.fn(), isPending: true },
    } as never;
    render(<ProviderCard {...defaultProps({ providerMutations: pendingMutations })} />);
    expect((screen.getByRole("button", { name: /Test Connection/i }) as HTMLButtonElement).disabled).toBe(true);
  });
});

// ─── Model create ────────────────────────────────────────────────────────
describe("ProviderCard model create", () => {
  it("shows inline create form when + Add Model clicked", () => {
    render(<ProviderCard {...defaultProps()} />);
    fireEvent.click(screen.getByRole("button", { name: /\+ Add Model/i }));
    expect(screen.getByPlaceholderText(/model name/i)).toBeDefined();
  });

  it("calls onModelCreate with derived payload on Save", () => {
    const onModelCreate = vi.fn();
    render(<ProviderCard {...defaultProps({ onModelCreate })} />);
    fireEvent.click(screen.getByRole("button", { name: /\+ Add Model/i }));
    fireEvent.change(screen.getByPlaceholderText(/model name/i), { target: { value: "deepseek-chat" } });
    fireEvent.click(screen.getByRole("button", { name: /^Save$/i }));
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
    render(<ProviderCard {...defaultProps({ onModelCreate })} />);
    fireEvent.click(screen.getByRole("button", { name: /\+ Add Model/i }));
    fireEvent.change(screen.getByPlaceholderText(/model name/i), { target: { value: "deepseek-chat" } });
    fireEvent.change(screen.getByPlaceholderText(/tags/i), { target: { value: "cheap, fast" } });
    fireEvent.click(screen.getByRole("button", { name: /^Save$/i }));
    expect(onModelCreate).toHaveBeenCalledWith(
      expect.objectContaining({ tags: ["cheap", "fast"] }),
    );
  });

  it("hides create form and clears draft when Cancel clicked", () => {
    render(<ProviderCard {...defaultProps()} />);
    fireEvent.click(screen.getByRole("button", { name: /\+ Add Model/i }));
    fireEvent.change(screen.getByPlaceholderText(/model name/i), { target: { value: "temp" } });
    fireEvent.click(screen.getByRole("button", { name: /^Cancel$/i }));
    expect(screen.queryByPlaceholderText(/model name/i)).toBeNull();
  });

  it("does not call onModelCreate when modelId is empty", () => {
    const onModelCreate = vi.fn();
    render(<ProviderCard {...defaultProps({ onModelCreate })} />);
    fireEvent.click(screen.getByRole("button", { name: /\+ Add Model/i }));
    fireEvent.click(screen.getByRole("button", { name: /^Save$/i }));
    expect(onModelCreate).not.toHaveBeenCalled();
  });

  it("shows fallback error when onModelCreate rejects with no message", async () => {
    // Non-Error object: err.message is undefined → triggers ?? fallback.
    const onModelCreate = vi.fn().mockRejectedValue({} as Error);
    render(<ProviderCard {...defaultProps({ onModelCreate })} />);
    fireEvent.click(screen.getByRole("button", { name: /\+ Add Model/i }));
    fireEvent.change(screen.getByPlaceholderText(/model name/i), { target: { value: "test-model" } });
    fireEvent.click(screen.getByRole("button", { name: /^Save$/i }));
    await vi.waitFor(() => {
      expect(screen.getByText(/Create failed/)).toBeDefined();
    });
  });
});

// ─── Model edit ──────────────────────────────────────────────────────────
describe("ProviderCard model edit", () => {
  const renderCardWithModel = (overrides: { onModelUpdate?: ReturnType<typeof vi.fn>; onModelTagsUpdate?: ReturnType<typeof vi.fn> } = {}) => {
    const onModelUpdate = overrides.onModelUpdate ?? vi.fn();
    const onModelTagsUpdate = overrides.onModelTagsUpdate ?? vi.fn();
    const result = render(
      <ProviderCard
        {...defaultProps({ models: [makeModel()], onModelUpdate, onModelTagsUpdate })}
      />,
    );
    return { ...result, onModelUpdate, onModelTagsUpdate };
  };

  it("shows edit form with current model values when Edit clicked", () => {
    renderCardWithModel();
    const editButtons = screen.getAllByRole("button", { name: /Edit/i });
    fireEvent.click(editButtons[editButtons.length - 1]);
    expect(screen.getByDisplayValue("deepseek-chat")).toBeDefined();
    expect(screen.getByDisplayValue("200000")).toBeDefined();
    expect(screen.getByDisplayValue("8192")).toBeDefined();
    expect((screen.getByRole("checkbox", { name: /reasoning/i }) as HTMLInputElement).checked).toBe(false);
  });

  it("calls onModelUpdate with only changed fields on save (single-state Option semantics)", () => {
    const { onModelUpdate } = renderCardWithModel();
    const editButtons = screen.getAllByRole("button", { name: /Edit/i });
    fireEvent.click(editButtons[editButtons.length - 1]);
    const nameInput = screen.getByDisplayValue("deepseek-chat");
    fireEvent.change(nameInput, { target: { value: "DeepSeek Chat" } });
    fireEvent.click(screen.getByRole("button", { name: /^Save$/i }));
    expect(onModelUpdate).toHaveBeenCalledWith(
      makeModel(),
      expect.objectContaining({
        id: "model-db-1",
        display_name: "DeepSeek Chat",
      }),
    );
    const call = onModelUpdate.mock.calls[0][1] as ModelUpdateRequestDto;
    // Unchanged fields are null (wire-compatible with serde None), not undefined.
    expect(call.context_window).toBeNull();
    expect(call.max_tokens).toBeNull();
    expect(call.reasoning).toBeNull();
    expect(call.enabled).toBeNull();
  });

  it("calls onModelTagsUpdate when tags changed on save", () => {
    const { onModelTagsUpdate } = renderCardWithModel();
    const editButtons = screen.getAllByRole("button", { name: /Edit/i });
    fireEvent.click(editButtons[editButtons.length - 1]);
    const tagsInput = screen.getByPlaceholderText(/tags/i);
    fireEvent.change(tagsInput, { target: { value: "cheap,fast" } });
    fireEvent.click(screen.getByRole("button", { name: /^Save$/i }));
    expect(onModelTagsUpdate).toHaveBeenCalledWith(makeModel(), ["cheap", "fast"]);
  });

  it("does not call onModelTagsUpdate when tags unchanged", () => {
    const { onModelTagsUpdate } = renderCardWithModel();
    const editButtons = screen.getAllByRole("button", { name: /Edit/i });
    fireEvent.click(editButtons[editButtons.length - 1]);
    fireEvent.click(screen.getByRole("button", { name: /^Save$/i }));
    expect(onModelTagsUpdate).not.toHaveBeenCalled();
  });

  it("exits edit mode on Cancel", () => {
    renderCardWithModel();
    const editButtons = screen.getAllByRole("button", { name: /Edit/i });
    fireEvent.click(editButtons[editButtons.length - 1]);
    fireEvent.click(screen.getByRole("button", { name: /Cancel/i }));
    expect(screen.queryByRole("checkbox", { name: /reasoning/i })).toBeNull();
  });

  it("does not clear display_name when edit input is empty (single-state Option semantics)", () => {
    const { onModelUpdate } = renderCardWithModel();
    const editButtons = screen.getAllByRole("button", { name: /Edit/i });
    fireEvent.click(editButtons[editButtons.length - 1]);
    const nameInput = screen.getByDisplayValue("deepseek-chat");
    fireEvent.change(nameInput, { target: { value: "" } });
    fireEvent.click(screen.getByRole("button", { name: /^Save$/i }));
    expect(onModelUpdate).not.toHaveBeenCalled();
  });

  it("includes context_window, max_tokens, reasoning, enabled in patch when changed", () => {
    const onModelUpdate = vi.fn().mockResolvedValue(undefined);
    const onModelTagsUpdate = vi.fn().mockResolvedValue(undefined);
    render(
      <ProviderCard
        {...defaultProps({ models: [makeModel()], onModelUpdate, onModelTagsUpdate })}
      />,
    );
    const editButtons = screen.getAllByRole("button", { name: /Edit/i });
    fireEvent.click(editButtons[editButtons.length - 1]);
    // Change all four fields.
    fireEvent.change(screen.getByDisplayValue("200000"), { target: { value: "128000" } });
    fireEvent.change(screen.getByDisplayValue("8192"), { target: { value: "4096" } });
    fireEvent.click(screen.getByRole("checkbox", { name: /reasoning/i }));
    fireEvent.click(screen.getByRole("checkbox", { name: /enabled/i }));
    fireEvent.click(screen.getByRole("button", { name: /^Save$/i }));
    expect(onModelUpdate).toHaveBeenCalledWith(
      makeModel(),
      expect.objectContaining({
        id: "model-db-1",
        context_window: 128000,
        max_tokens: 4096,
        reasoning: true,
        enabled: false,
      }),
    );
  });

  it("shows fallback error message when onModelUpdate rejects with no message", async () => {
    // Non-Error object: err.message is undefined → triggers ?? fallback.
    const onModelUpdate = vi.fn().mockRejectedValue({} as Error);
    const onModelTagsUpdate = vi.fn().mockResolvedValue(undefined);
    render(
      <ProviderCard
        {...defaultProps({ models: [makeModel()], onModelUpdate, onModelTagsUpdate })}
      />,
    );
    const editButtons = screen.getAllByRole("button", { name: /Edit/i });
    fireEvent.click(editButtons[editButtons.length - 1]);
    const nameInput = screen.getByDisplayValue("deepseek-chat");
    fireEvent.change(nameInput, { target: { value: "changed" } });
    fireEvent.click(screen.getByRole("button", { name: /^Save$/i }));
    await vi.waitFor(() => {
      expect(screen.getByText(/Save failed/)).toBeDefined();
    });
  });
});

// ─── Provider edit mode ──────────────────────────────────────────────────
describe("ProviderCard edit mode", () => {
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
        {...defaultProps({
          models: [makeModel()],
          providerMutations,
          isEditing: true,
          onCancelEdit,
        })}
      />,
    );
    return { updateProvider, onCancelEdit };
  };

  it("renders editable inputs for base url, api key, kind, name when isEditing", () => {
    renderCardInEditMode();
    expect(screen.getByDisplayValue("https://api.deepseek.com/v1")).toBeDefined();
    expect(screen.getByDisplayValue("deepseek_openai")).toBeDefined();
    expect(screen.getByPlaceholderText(/new api key/i)).toBeDefined();
  });

  it("disables model operation buttons when editing", () => {
    renderCardInEditMode();
    const addModelButton = screen.getByRole("button", { name: /\+ Add Model/i }) as HTMLButtonElement;
    expect(addModelButton.disabled).toBe(true);
  });

  it("shows notice when editing", () => {
    renderCardInEditMode();
    expect(screen.getByText(/Editing provider details/i)).toBeDefined();
  });

  it("calls updateProvider with patch on Save", () => {
    const { updateProvider } = renderCardInEditMode();
    fireEvent.change(screen.getByDisplayValue("https://api.deepseek.com/v1"), { target: { value: "https://api.deepseek.com/v2" } });
    fireEvent.click(screen.getByRole("button", { name: /^Save$/i }));
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
    fireEvent.click(screen.getByRole("button", { name: /^Save$/i }));
    const call = (updateProvider as unknown as { mock: { calls: unknown[][] } }).mock.calls[0][0] as Record<string, unknown>;
    expect(call.api_key).toBeUndefined();
  });

  it("includes api_key in patch when key field is filled", () => {
    const { updateProvider } = renderCardInEditMode();
    fireEvent.change(screen.getByPlaceholderText(/new api key/i), { target: { value: "sk-new" } });
    fireEvent.click(screen.getByRole("button", { name: /^Save$/i }));
    const call = (updateProvider as unknown as { mock: { calls: unknown[][] } }).mock.calls[0][0] as Record<string, unknown>;
    expect(call.api_key).toBe("sk-new");
  });

  it("calls onCancelEdit on Cancel button click", () => {
    const onCancelEdit = vi.fn();
    renderCardInEditMode({ onCancelEdit });
    fireEvent.click(screen.getByRole("button", { name: /^Cancel$/i }));
    expect(onCancelEdit).toHaveBeenCalledOnce();
  });

  it("shows provider save error when updateProvider fails (f3)", () => {
    const updateProvider = vi.fn((_payload: unknown, opts?: { onError?: (e: Error) => void }) => {
      opts?.onError?.(new Error("update rpc failed"));
    });
    renderCardInEditMode({ updateProvider });
    fireEvent.click(screen.getByRole("button", { name: /^Save$/i }));
    expect(screen.getByText(/update rpc failed/)).toBeDefined();
  });

  it("shows fallback error when updateProvider fails with no message", () => {
    const updateProvider = vi.fn((_payload: unknown, opts?: { onError?: (e: Error) => void }) => {
      // Non-Error object: err.message is undefined → triggers ?? fallback.
      opts?.onError?.({} as Error);
    });
    renderCardInEditMode({ updateProvider });
    fireEvent.click(screen.getByRole("button", { name: /^Save$/i }));
    expect(screen.getByText(/Save failed/)).toBeDefined();
  });

  it("updates provider name and kind via edit form inputs", () => {
    const { updateProvider } = renderCardInEditMode();
    fireEvent.change(screen.getByDisplayValue("deepseek_openai"), { target: { value: "my_provider" } });
    fireEvent.change(screen.getByDisplayValue("OpenAI-compatible"), { target: { value: "anthropic_compatible" } });
    fireEvent.click(screen.getByRole("button", { name: /^Save$/i }));
    const call = (updateProvider as unknown as { mock: { calls: unknown[][] } }).mock.calls[0][0] as Record<string, unknown>;
    expect(call.name).toBe("my_provider");
    expect(call.provider_kind).toBe("anthropic_compatible");
  });

  it("disables save/cancel buttons when updateProvider.isPending (f4)", () => {
    const providerMutations = {
      createProvider: { mutate: vi.fn(), isPending: false },
      updateProvider: { mutate: vi.fn(), isPending: true },
      deleteProvider: { mutate: vi.fn(), isPending: false },
      testConnection: { mutate: vi.fn(), isPending: false },
    } as never;
    render(
      <ProviderCard
        {...defaultProps({
          models: [makeModel()],
          providerMutations,
          isEditing: true,
        })}
      />,
    );
    expect((screen.getByRole("button", { name: /^Save$/i }) as HTMLButtonElement).disabled).toBe(true);
    expect((screen.getByRole("button", { name: /^Cancel$/i }) as HTMLButtonElement).disabled).toBe(true);
  });

  it("sets aria-invalid and shows field-error when base URL is invalid on blur (P2 #7)", () => {
    renderCardInEditMode();
    const urlInput = screen.getByDisplayValue("https://api.deepseek.com/v1");
    fireEvent.change(urlInput, { target: { value: "not-a-url" } });
    fireEvent.blur(urlInput);
    // aria-invalid reflects the error state.
    expect((urlInput as HTMLInputElement).getAttribute("aria-invalid")).toBe("true");
    // Error message renders in a role=alert div.
    expect(screen.getByRole("alert").textContent).toContain("http://");
  });

  it("clears aria-invalid and field-error when base URL is corrected on blur", () => {
    renderCardInEditMode();
    const urlInput = screen.getByDisplayValue("https://api.deepseek.com/v1");
    // First make it invalid.
    fireEvent.change(urlInput, { target: { value: "bad" } });
    fireEvent.blur(urlInput);
    expect(screen.getByRole("alert")).toBeDefined();
    // Now fix it.
    fireEvent.change(urlInput, { target: { value: "https://api.openai.com/v1" } });
    fireEvent.blur(urlInput);
    expect((urlInput as HTMLInputElement).getAttribute("aria-invalid")).toBe("false");
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("clears providerSaveError when re-entering edit mode (P2 #9)", () => {
    // 1. Mount in edit mode, trigger a save failure → error shows.
    const updateProvider = vi.fn((_payload: unknown, opts?: { onError?: (e: Error) => void }) => {
      opts?.onError?.(new Error("update rpc failed"));
    });
    const { rerender } = render(
      <ProviderCard
        {...defaultProps({
          models: [makeModel()],
          providerMutations: {
            createProvider: { mutate: vi.fn(), isPending: false },
            updateProvider: { mutate: updateProvider, isPending: false },
            deleteProvider: { mutate: vi.fn(), isPending: false },
            testConnection: { mutate: vi.fn(), isPending: false },
          } as never,
          isEditing: true,
          onCancelEdit: vi.fn(),
        })}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: /^Save$/i }));
    expect(screen.getByText(/update rpc failed/)).toBeDefined();
    // 2. Exit edit mode (parent sets isEditing=false).
    rerender(
      <ProviderCard
        {...defaultProps({
          models: [makeModel()],
          providerMutations: {
            createProvider: { mutate: vi.fn(), isPending: false },
            updateProvider: { mutate: updateProvider, isPending: false },
            deleteProvider: { mutate: vi.fn(), isPending: false },
            testConnection: { mutate: vi.fn(), isPending: false },
          } as never,
          isEditing: false,
          onCancelEdit: vi.fn(),
        })}
      />,
    );
    // 3. Re-enter edit mode → prevEditing pattern should clear the error.
    rerender(
      <ProviderCard
        {...defaultProps({
          models: [makeModel()],
          providerMutations: {
            createProvider: { mutate: vi.fn(), isPending: false },
            updateProvider: { mutate: updateProvider, isPending: false },
            deleteProvider: { mutate: vi.fn(), isPending: false },
            testConnection: { mutate: vi.fn(), isPending: false },
          } as never,
          isEditing: true,
          onCancelEdit: vi.fn(),
        })}
      />,
    );
    expect(screen.queryByText(/update rpc failed/)).toBeNull();
  });
});

// ─── Save success feedback ───────────────────────────────────────────────
describe("ProviderCard save success feedback", () => {
  it("shows Saved banner in view mode after successful save", () => {
    const updateProvider = vi.fn((_payload: unknown, opts?: { onSuccess?: () => void }) => {
      opts?.onSuccess?.();
    });
    const onCancelEdit = vi.fn();
    const providerMutations = {
      createProvider: { mutate: vi.fn(), isPending: false },
      updateProvider: { mutate: updateProvider, isPending: false },
      deleteProvider: { mutate: vi.fn(), isPending: false },
      testConnection: { mutate: vi.fn(), isPending: false },
    } as never;
    const { rerender } = render(
      <ProviderCard
        {...defaultProps({
          models: [makeModel()],
          providerMutations,
          isEditing: true,
          onCancelEdit,
        })}
      />,
    );
    // Make a change (dirty the form), then save.
    fireEvent.change(screen.getByDisplayValue("https://api.deepseek.com/v1"), {
      target: { value: "https://api.deepseek.com/v2" },
    });
    fireEvent.click(screen.getByRole("button", { name: /^Save$/i }));
    // Parent would set isEditing=false after onCancelEdit.
    rerender(
      <ProviderCard
        {...defaultProps({
          models: [makeModel({})],
          providerMutations,
          isEditing: false,
          onCancelEdit,
        })}
      />,
    );
    expect(screen.getByText("Saved")).toBeDefined();
  });

  it("clears Saved banner when re-entering edit mode", () => {
    const updateProvider = vi.fn((_payload: unknown, opts?: { onSuccess?: () => void }) => {
      opts?.onSuccess?.();
    });
    const providerMutations = {
      createProvider: { mutate: vi.fn(), isPending: false },
      updateProvider: { mutate: updateProvider, isPending: false },
      deleteProvider: { mutate: vi.fn(), isPending: false },
      testConnection: { mutate: vi.fn(), isPending: false },
    } as never;
    const { rerender } = render(
      <ProviderCard
        {...defaultProps({
          models: [makeModel()],
          providerMutations,
          isEditing: true,
          onCancelEdit: vi.fn(),
        })}
      />,
    );
    fireEvent.change(screen.getByDisplayValue("https://api.deepseek.com/v1"), {
      target: { value: "https://api.deepseek.com/v2" },
    });
    fireEvent.click(screen.getByRole("button", { name: /^Save$/i }));
    // Exit edit mode → success banner shows.
    rerender(
      <ProviderCard
        {...defaultProps({
          models: [makeModel()],
          providerMutations,
          isEditing: false,
          onCancelEdit: vi.fn(),
        })}
      />,
    );
    expect(screen.getByText("Saved")).toBeDefined();
    // Re-enter edit mode → banner cleared.
    rerender(
      <ProviderCard
        {...defaultProps({
          models: [makeModel()],
          providerMutations,
          isEditing: true,
          onCancelEdit: vi.fn(),
        })}
      />,
    );
    expect(screen.queryByText("Saved")).toBeNull();
  });
});

// ─── Dirty form protection ───────────────────────────────────────────────
describe("ProviderCard dirty form protection", () => {
  it("shows confirm dialog when canceling provider edit with unsaved changes", () => {
    const onCancelEdit = vi.fn();
    const providerMutations = {
      createProvider: { mutate: vi.fn(), isPending: false },
      updateProvider: { mutate: vi.fn(), isPending: false },
      deleteProvider: { mutate: vi.fn(), isPending: false },
      testConnection: { mutate: vi.fn(), isPending: false },
    } as never;
    render(
      <ProviderCard
        {...defaultProps({
          models: [makeModel()],
          providerMutations,
          isEditing: true,
          onCancelEdit,
        })}
      />,
    );
    // Change the name → dirty.
    fireEvent.change(screen.getByDisplayValue("deepseek_openai"), {
      target: { value: "new_name" },
    });
    fireEvent.click(screen.getByRole("button", { name: /^Cancel$/i }));
    // Confirm dialog should be open.
    expect(screen.getByText("Discard Changes")).toBeDefined();
    expect(onCancelEdit).not.toHaveBeenCalled();
  });

  it("exits edit mode immediately when canceling with no changes", () => {
    const onCancelEdit = vi.fn();
    const providerMutations = {
      createProvider: { mutate: vi.fn(), isPending: false },
      updateProvider: { mutate: vi.fn(), isPending: false },
      deleteProvider: { mutate: vi.fn(), isPending: false },
      testConnection: { mutate: vi.fn(), isPending: false },
    } as never;
    render(
      <ProviderCard
        {...defaultProps({
          models: [makeModel()],
          providerMutations,
          isEditing: true,
          onCancelEdit,
        })}
      />,
    );
    // No changes → cancel should exit immediately.
    fireEvent.click(screen.getByRole("button", { name: /^Cancel$/i }));
    expect(onCancelEdit).toHaveBeenCalledOnce();
    expect(screen.queryByText("Discard Changes")).toBeNull();
  });

  it("discards changes and exits when confirm dialog confirmed (provider edit)", () => {
    const onCancelEdit = vi.fn();
    const providerMutations = {
      createProvider: { mutate: vi.fn(), isPending: false },
      updateProvider: { mutate: vi.fn(), isPending: false },
      deleteProvider: { mutate: vi.fn(), isPending: false },
      testConnection: { mutate: vi.fn(), isPending: false },
    } as never;
    render(
      <ProviderCard
        {...defaultProps({
          models: [makeModel()],
          providerMutations,
          isEditing: true,
          onCancelEdit,
        })}
      />,
    );
    fireEvent.change(screen.getByDisplayValue("deepseek_openai"), {
      target: { value: "new_name" },
    });
    fireEvent.click(screen.getByRole("button", { name: /^Cancel$/i }));
    // Confirm discard.
    fireEvent.click(screen.getByRole("button", { name: /^Discard$/i }));
    expect(onCancelEdit).toHaveBeenCalledOnce();
  });

  it("shows confirm dialog when canceling model edit with unsaved changes", () => {
    render(<ProviderCard {...defaultProps({ models: [makeModel()] })} />);
    // Enter model edit.
    const editButtons = screen.getAllByRole("button", { name: /Edit/i });
    fireEvent.click(editButtons[editButtons.length - 1]);
    // Change display name → dirty.
    fireEvent.change(screen.getByDisplayValue("deepseek-chat"), {
      target: { value: "New Name" },
    });
    // Click cancel (the model edit cancel button).
    const cancelButtons = screen.getAllByRole("button", { name: /^Cancel$/i });
    fireEvent.click(cancelButtons[cancelButtons.length - 1]);
    // Confirm dialog should be open.
    expect(screen.getByText("Discard Changes")).toBeDefined();
    // Model edit form should still be visible (not yet discarded).
    expect(screen.getByDisplayValue("New Name")).toBeDefined();
  });

  it("exits model edit immediately when canceling with no changes", () => {
    render(<ProviderCard {...defaultProps({ models: [makeModel()] })} />);
    const editButtons = screen.getAllByRole("button", { name: /Edit/i });
    fireEvent.click(editButtons[editButtons.length - 1]);
    // No changes → cancel should exit immediately.
    fireEvent.click(screen.getByRole("button", { name: /^Cancel$/i }));
    expect(screen.queryByText("Discard Changes")).toBeNull();
    expect(screen.queryByRole("checkbox", { name: /reasoning/i })).toBeNull();
  });

  it("discards model edit changes and exits when confirm dialog confirmed", () => {
    render(<ProviderCard {...defaultProps({ models: [makeModel()] })} />);
    const editButtons = screen.getAllByRole("button", { name: /Edit/i });
    fireEvent.click(editButtons[editButtons.length - 1]);
    fireEvent.change(screen.getByDisplayValue("deepseek-chat"), {
      target: { value: "Changed" },
    });
    fireEvent.click(screen.getByRole("button", { name: /^Cancel$/i }));
    // Confirm discard.
    fireEvent.click(screen.getByRole("button", { name: /^Discard$/i }));
    expect(screen.queryByText("Discard Changes")).toBeNull();
    expect(screen.queryByDisplayValue("Changed")).toBeNull();
  });

  it("does not show stale deleteError in cancel-confirm dialog", async () => {
    // 1. Trigger a model delete that fails → deleteError is set.
    const onModelDelete = vi.fn().mockRejectedValue(new Error("delete rpc failed"));
    render(
      <ProviderCard {...defaultProps({ models: [makeModel()], onModelDelete })} />,
    );
    const deleteButtons = screen.getAllByRole("button", { name: /Delete/i });
    fireEvent.click(deleteButtons[deleteButtons.length - 1]);
    // Confirm delete → onModelDelete rejects → deleteError shows in dialog.
    const dialogDeleteButtons = screen.getAllByRole("button", { name: /Delete/i });
    fireEvent.click(dialogDeleteButtons[dialogDeleteButtons.length - 1]);
    await vi.waitFor(() => {
      expect(screen.getByText(/delete rpc failed/)).toBeDefined();
    });
    // 2. Cancel the delete dialog.
    fireEvent.click(screen.getByRole("button", { name: /cancel/i }));
    expect(screen.queryByText(/delete rpc failed/)).toBeNull();
    // 3. Enter model edit, dirty it, cancel → cancel-confirm dialog opens.
    const editButtons = screen.getAllByRole("button", { name: /Edit/i });
    fireEvent.click(editButtons[editButtons.length - 1]);
    fireEvent.change(screen.getByDisplayValue("deepseek-chat"), {
      target: { value: "Changed" },
    });
    fireEvent.click(screen.getByRole("button", { name: /^Cancel$/i }));
    // 4. The cancel-confirm dialog should NOT show the stale deleteError.
    expect(screen.getByText("Discard Changes")).toBeDefined();
    expect(screen.queryByText(/delete rpc failed/)).toBeNull();
  });
});

// ─── aria-describedby association ────────────────────────────────────────
describe("ProviderCard aria-describedby for edit URL error", () => {
  it("links URL input to error element via aria-describedby when error is present", () => {
    const providerMutations = {
      createProvider: { mutate: vi.fn(), isPending: false },
      updateProvider: { mutate: vi.fn(), isPending: false },
      deleteProvider: { mutate: vi.fn(), isPending: false },
      testConnection: { mutate: vi.fn(), isPending: false },
    } as never;
    render(
      <ProviderCard
        {...defaultProps({
          models: [makeModel()],
          providerMutations,
          isEditing: true,
          onCancelEdit: vi.fn(),
        })}
      />,
    );
    const urlInput = screen.getByDisplayValue("https://api.deepseek.com/v1");
    fireEvent.change(urlInput, { target: { value: "bad" } });
    fireEvent.blur(urlInput);
    // aria-describedby should reference the error element's id.
    const describedBy = urlInput.getAttribute("aria-describedby");
    expect(describedBy).toBeTruthy();
    const errorEl = document.getElementById(describedBy!);
    expect(errorEl).toBeTruthy();
    expect(errorEl!.getAttribute("role")).toBe("alert");
  });

  it("does not set aria-describedby when there is no URL error", () => {
    const providerMutations = {
      createProvider: { mutate: vi.fn(), isPending: false },
      updateProvider: { mutate: vi.fn(), isPending: false },
      deleteProvider: { mutate: vi.fn(), isPending: false },
      testConnection: { mutate: vi.fn(), isPending: false },
    } as never;
    render(
      <ProviderCard
        {...defaultProps({
          models: [makeModel()],
          providerMutations,
          isEditing: true,
          onCancelEdit: vi.fn(),
        })}
      />,
    );
    const urlInput = screen.getByDisplayValue("https://api.deepseek.com/v1");
    expect(urlInput.getAttribute("aria-describedby")).toBeNull();
  });
});

// ─── Autocomplete attribute ──────────────────────────────────────────────
describe("ProviderCard autocomplete on API key input", () => {
  it("sets autoComplete=off on the API key input in edit mode", () => {
    const providerMutations = {
      createProvider: { mutate: vi.fn(), isPending: false },
      updateProvider: { mutate: vi.fn(), isPending: false },
      deleteProvider: { mutate: vi.fn(), isPending: false },
      testConnection: { mutate: vi.fn(), isPending: false },
    } as never;
    render(
      <ProviderCard
        {...defaultProps({
          models: [makeModel()],
          providerMutations,
          isEditing: true,
          onCancelEdit: vi.fn(),
        })}
      />,
    );
    const apiKeyInput = screen.getByPlaceholderText(/new api key/i);
    expect(apiKeyInput.getAttribute("autocomplete")).toBe("off");
  });
});
