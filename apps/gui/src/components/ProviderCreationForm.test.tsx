import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { ProviderDto, ProviderListResponseDto } from "@busytok/protocol-types";
import { ProviderCreationForm } from "./ProviderCreationForm";

vi.mock("../api/useBusytokData", () => ({
  useProviders: vi.fn(),
  useProviderMutations: vi.fn(),
  useModelMutations: vi.fn(),
}));
vi.mock("../logging/safeReporter", () => ({
  reportFrontendEventSafely: vi.fn(),
}));

import { useModelMutations, useProviderMutations, useProviders } from "../api/useBusytokData";
import { reportFrontendEventSafely } from "../logging/safeReporter";

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

const makeProvider = (overrides: Partial<ProviderDto> = {}): ProviderDto => ({
  id: "prov-new",
  name: "deepseek_openai",
  provider_kind: "openai_compatible" as never,
  base_url: "https://api.deepseek.com/v1",
  enabled: true,
  has_api_key: true,
  created_at_ms: 0,
  updated_at_ms: 0,
  ...overrides,
});

function renderForm(
  overrides: {
    existingNames?: string[];
    createProvider?: any;
    createModel?: any;
    createProviderPending?: boolean;
    createModelPending?: boolean;
  } = {},
) {
  const mockUseProviders = vi.mocked(useProviders);
  mockUseProviders.mockReturnValue({
    data: {
      providers: (overrides.existingNames ?? []).map((n) => makeProvider({ name: n })),
    } as ProviderListResponseDto,
    isLoading: false,
    isError: false,
    isFetching: false,
  } as never);

  // The `overrides.createProvider` / `createModel` args are vi.fn stand-ins
  // for the `.mutate` method of TanStack Query's UseMutationResult. They
  // are wrapped into the `{ mutate, isPending }` shape that the form
  // consumes via `useProviderMutations().createProvider.mutate(...)`.
  const createProviderMutate =
    overrides.createProvider ??
    vi.fn((_payload: unknown, opts?: { onSuccess?: (p: ProviderDto) => void }) => {
      opts?.onSuccess?.(makeProvider());
    });
  const createModelMutate =
    overrides.createModel ??
    vi.fn((_payload: unknown, opts?: { onSuccess?: () => void; onError?: (e: Error) => void }) => {
      opts?.onSuccess?.();
    });

  vi.mocked(useProviderMutations).mockReturnValue({
    createProvider: { mutate: createProviderMutate, isPending: overrides.createProviderPending ?? false },
    updateProvider: { mutate: vi.fn(), isPending: false },
    deleteProvider: { mutate: vi.fn(), isPending: false },
    testConnection: { mutate: vi.fn(), isPending: false },
  } as never);
  vi.mocked(useModelMutations).mockReturnValue({
    createModel: { mutate: createModelMutate, isPending: overrides.createModelPending ?? false },
    updateModel: { mutate: vi.fn(), isPending: false },
    deleteModel: { mutate: vi.fn(), isPending: false },
    tagsUpdate: { mutate: vi.fn(), isPending: false },
  } as never);

  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <ProviderCreationForm onClose={vi.fn()} />
    </QueryClientProvider>,
  );
}

function fillForm() {
  fireEvent.change(screen.getByPlaceholderText(/base url/i), { target: { value: "https://api.deepseek.com/v1" } });
  fireEvent.change(screen.getByPlaceholderText(/api key/i), { target: { value: "sk-test" } });
}

describe("ProviderCreationForm", () => {
  it("validates base URL on blur", () => {
    renderForm();
    const urlInput = screen.getByPlaceholderText(/base url/i);
    fireEvent.change(urlInput, { target: { value: "bad-url" } });
    fireEvent.blur(urlInput);
    expect(screen.getByText(/请输入完整的 URL/i)).toBeDefined();
  });

  it("disables Save when API key is empty", () => {
    renderForm();
    fireEvent.change(screen.getByPlaceholderText(/base url/i), { target: { value: "https://api.deepseek.com/v1" } });
    const saveBtn = screen.getByRole("button", { name: /^保存$/i }) as HTMLButtonElement;
    expect(saveBtn.disabled).toBe(true);
  });

  it("calls createProvider with derived name on Save (no model)", () => {
    const createProvider = vi.fn((_payload: unknown, opts?: { onSuccess?: (p: ProviderDto) => void }) => {
      opts?.onSuccess?.(makeProvider());
    });
    renderForm({ createProvider });
    fillForm();
    fireEvent.click(screen.getByRole("button", { name: /^保存$/i }));
    expect(createProvider).toHaveBeenCalledWith(
      expect.objectContaining({
        name: "deepseek_openai",
        provider_kind: "openai_compatible",
        base_url: "https://api.deepseek.com/v1",
        api_key: "sk-test",
        enabled: true,
      }),
      expect.anything(),
    );
  });

  it("derives name with _2 suffix on collision", () => {
    const createProvider = vi.fn((_payload: unknown, opts?: { onSuccess?: (p: ProviderDto) => void }) => {
      opts?.onSuccess?.(makeProvider({ name: "deepseek_openai_2" }));
    });
    renderForm({ existingNames: ["deepseek_openai"], createProvider });
    fillForm();
    fireEvent.click(screen.getByRole("button", { name: /^保存$/i }));
    expect(createProvider).toHaveBeenCalledWith(
      expect.objectContaining({ name: "deepseek_openai_2" }),
      expect.anything(),
    );
  });

  it("calls createModel after createProvider when model name is filled", () => {
    const createProvider = vi.fn((_payload: unknown, opts?: { onSuccess?: (p: ProviderDto) => void }) => {
      opts?.onSuccess?.(makeProvider());
    });
    const createModel = vi.fn((_payload: unknown, opts?: { onSuccess?: () => void }) => {
      opts?.onSuccess?.();
    });
    renderForm({ createProvider, createModel });
    fillForm();
    fireEvent.change(screen.getByPlaceholderText(/model name/i), { target: { value: "deepseek-chat" } });
    fireEvent.click(screen.getByRole("button", { name: /^保存$/i }));
    expect(createModel).toHaveBeenCalledWith(
      expect.objectContaining({
        provider_id: "prov-new",
        model_id: "deepseek-chat",
        display_name: "deepseek-chat",
        context_window: 200000,
        max_tokens: 8192,
        reasoning: true,
        enabled: true,
        tags: [],
      }),
      expect.anything(),
    );
  });

  it("enters partial-success state when createModel fails after createProvider succeeds", async () => {
    const createProvider = vi.fn((_payload: unknown, opts?: { onSuccess?: (p: ProviderDto) => void }) => {
      opts?.onSuccess?.(makeProvider());
    });
    const createModel = vi.fn((_payload: unknown, opts?: { onError?: (e: Error) => void }) => {
      opts?.onError?.(new Error("model already exists"));
    });
    renderForm({ createProvider, createModel });
    fillForm();
    fireEvent.change(screen.getByPlaceholderText(/model name/i), { target: { value: "deepseek-chat" } });
    fireEvent.click(screen.getByRole("button", { name: /^保存$/i }));

    await waitFor(() => {
      expect(screen.getByText(/model already exists/i)).toBeDefined();
    });
    // Save button should be disabled to prevent duplicate provider creation
    const saveBtn = screen.getByRole("button", { name: /^保存$/i }) as HTMLButtonElement;
    expect(saveBtn.disabled).toBe(true);
    // Retry button should be enabled
    const retryBtn = screen.getByRole("button", { name: /重试 model/i }) as HTMLButtonElement;
    expect(retryBtn.disabled).toBe(false);
  });

  it("emits provider.added and model.add.failed events on partial success", async () => {
    const createProvider = vi.fn((_payload: unknown, opts?: { onSuccess?: (p: ProviderDto) => void }) => {
      opts?.onSuccess?.(makeProvider());
    });
    const createModel = vi.fn((_payload: unknown, opts?: { onError?: (e: Error) => void }) => {
      opts?.onError?.(new Error("model already exists"));
    });
    renderForm({ createProvider, createModel });
    fillForm();
    fireEvent.change(screen.getByPlaceholderText(/model name/i), { target: { value: "deepseek-chat" } });
    fireEvent.click(screen.getByRole("button", { name: /^保存$/i }));

    await waitFor(() => {
      expect(vi.mocked(reportFrontendEventSafely)).toHaveBeenCalledWith(
        expect.objectContaining({ event_code: "provider.added" }),
      );
    });
    await waitFor(() => {
      expect(vi.mocked(reportFrontendEventSafely)).toHaveBeenCalledWith(
        expect.objectContaining({ event_code: "model.add.failed" }),
      );
    });
  });

  it("retries only createModel (not createProvider) on partial-success retry", async () => {
    const createProvider = vi.fn((_payload: unknown, opts?: { onSuccess?: (p: ProviderDto) => void }) => {
      opts?.onSuccess?.(makeProvider());
    });
    const createModel = vi.fn((_payload: unknown, opts?: { onError?: (e: Error) => void; onSuccess?: () => void }) => {
      // First call fails, second succeeds
      if ((createModel as any).mock.calls.length === 1) {
        opts?.onError?.(new Error("model already exists"));
      } else {
        opts?.onSuccess?.();
      }
    });
    renderForm({ createProvider, createModel });
    fillForm();
    fireEvent.change(screen.getByPlaceholderText(/model name/i), { target: { value: "deepseek-chat" } });
    fireEvent.click(screen.getByRole("button", { name: /^保存$/i }));

    await waitFor(() => expect(screen.getByRole("button", { name: /重试 model/i })).toBeDefined());
    fireEvent.click(screen.getByRole("button", { name: /重试 model/i }));

    // createProvider should only have been called once
    expect(createProvider).toHaveBeenCalledTimes(1);
    // createModel should have been called twice
    await waitFor(() => expect(createModel).toHaveBeenCalledTimes(2));
  });

  it("emits provider.add.failed when createProvider fails", async () => {
    const createProvider = vi.fn((_payload: unknown, opts?: { onError?: (e: Error) => void }) => {
      opts?.onError?.(new Error("network error"));
    });
    renderForm({ createProvider });
    fillForm();
    fireEvent.click(screen.getByRole("button", { name: /^保存$/i }));

    await waitFor(() => {
      expect(vi.mocked(reportFrontendEventSafely)).toHaveBeenCalledWith(
        expect.objectContaining({ event_code: "provider.add.failed" }),
      );
    });
  });

  // ─── f2: aria-invalid on URL input ──────────────────────────────────
  it("sets aria-invalid=true on URL input when validation fails", () => {
    renderForm();
    const urlInput = screen.getByPlaceholderText(/base url/i);
    fireEvent.change(urlInput, { target: { value: "bad-url" } });
    fireEvent.blur(urlInput);
    expect(urlInput.getAttribute("aria-invalid")).toBe("true");
  });

  it("sets aria-invalid=false on URL input when validation passes", () => {
    renderForm();
    const urlInput = screen.getByPlaceholderText(/base url/i);
    fireEvent.change(urlInput, { target: { value: "https://api.deepseek.com/v1" } });
    fireEvent.blur(urlInput);
    expect(urlInput.getAttribute("aria-invalid")).toBe("false");
  });

  it("uses role=alert for URL validation error (live region)", () => {
    renderForm();
    const urlInput = screen.getByPlaceholderText(/base url/i);
    fireEvent.change(urlInput, { target: { value: "bad-url" } });
    fireEvent.blur(urlInput);
    expect(screen.getByRole("alert")).toBeDefined();
  });

  // ─── f4: disable on mutation pending ───────────────────────────────
  it("disables Save when createProvider.isPending is true", () => {
    renderForm({ createProviderPending: true });
    fillForm();
    const saveBtn = screen.getByRole("button", { name: /^保存$/i }) as HTMLButtonElement;
    expect(saveBtn.disabled).toBe(true);
  });

  it("disables Save when createModel.isPending is true", () => {
    renderForm({ createModelPending: true });
    fillForm();
    const saveBtn = screen.getByRole("button", { name: /^保存$/i }) as HTMLButtonElement;
    expect(saveBtn.disabled).toBe(true);
  });

  // ─── Kind select + model tags inputs (cover onChange handlers) ──────
  it("changes provider kind via select dropdown", () => {
    const createProvider = vi.fn((_payload: unknown, opts?: { onSuccess?: (p: ProviderDto) => void }) => {
      opts?.onSuccess?.(makeProvider());
    });
    renderForm({ createProvider });
    fillForm();
    fireEvent.change(screen.getByLabelText(/kind/i), { target: { value: "anthropic_compatible" } });
    fireEvent.click(screen.getByRole("button", { name: /^保存$/i }));
    expect(createProvider).toHaveBeenCalledWith(
      expect.objectContaining({ provider_kind: "anthropic_compatible" }),
      expect.anything(),
    );
  });

  it("passes parsed model tags to createModel", () => {
    const createProvider = vi.fn((_payload: unknown, opts?: { onSuccess?: (p: ProviderDto) => void }) => {
      opts?.onSuccess?.(makeProvider());
    });
    const createModel = vi.fn((_payload: unknown, opts?: { onSuccess?: () => void }) => {
      opts?.onSuccess?.();
    });
    renderForm({ createProvider, createModel });
    fillForm();
    fireEvent.change(screen.getByPlaceholderText(/model name/i), { target: { value: "claude-3" } });
    fireEvent.change(screen.getByPlaceholderText(/tags/i), { target: { value: "fast, expensive" } });
    fireEvent.click(screen.getByRole("button", { name: /^保存$/i }));
    expect(createModel).toHaveBeenCalledWith(
      expect.objectContaining({ tags: ["fast", "expensive"] }),
      expect.anything(),
    );
  });

  // ─── Retry model failure (cover onError at handleRetryModel) ────────
  it("stays in partial-success when model retry fails again", async () => {
    const createProvider = vi.fn((_payload: unknown, opts?: { onSuccess?: (p: ProviderDto) => void }) => {
      opts?.onSuccess?.(makeProvider());
    });
    const createModel = vi.fn((_payload: unknown, opts?: { onError?: (e: Error) => void; onSuccess?: () => void }) => {
      // Both first attempt and retry fail.
      opts?.onError?.(new Error("model conflict"));
    });
    renderForm({ createProvider, createModel });
    fillForm();
    fireEvent.change(screen.getByPlaceholderText(/model name/i), { target: { value: "deepseek-chat" } });
    fireEvent.click(screen.getByRole("button", { name: /^保存$/i }));

    // Wait for partial-success state + retry button.
    await waitFor(() => expect(screen.getByRole("button", { name: /重试 model/i })).toBeDefined());
    fireEvent.click(screen.getByRole("button", { name: /重试 model/i }));

    // Retry failed → still in partial-success, error banner still shows.
    await waitFor(() => {
      expect(screen.getByText(/model conflict/)).toBeDefined();
    });
    expect(screen.getByRole("button", { name: /重试 model/i })).toBeDefined();
  });

  // ─── providersQuery.data null fallback (branch at line 54) ──────────
  it("handles null providersQuery.data gracefully (no existing names)", () => {
    const createProvider = vi.fn((_payload: unknown, opts?: { onSuccess?: (p: ProviderDto) => void }) => {
      opts?.onSuccess?.(makeProvider());
    });
    renderForm({ createProvider });
    // Override useProviders AFTER renderForm (which sets data to { providers: [] }).
    // The next state change (fillForm) triggers a re-render that picks up the new mock.
    vi.mocked(useProviders).mockReturnValue({ data: undefined } as never);
    fillForm();
    fireEvent.click(screen.getByRole("button", { name: /^保存$/i }));
    // Name derived from URL without collision suffix (existingNames falls back to empty Set).
    expect(createProvider).toHaveBeenCalledWith(
      expect.objectContaining({ name: "deepseek_openai" }),
      expect.anything(),
    );
  });
});
