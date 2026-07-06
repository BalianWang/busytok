import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, render, screen, fireEvent } from "@testing-library/react";
import type { ProviderDto, ModelCatalogEntryDto } from "@busytok/protocol-types";
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
  tags: ["cheap", "fast"],
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
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: /删除/i }));
    expect(onDelete).not.toHaveBeenCalled();
    confirmSpy.mockRestore();
  });
});
